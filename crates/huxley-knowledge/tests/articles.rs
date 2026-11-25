//! Articles against a real PostgreSQL: revisions, the review flow, what
//! readers see, search, and organization scope.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_knowledge::KbError;
use huxley_knowledge::articles::{self, ArticleChanges, ArticleInput, Drafts, Filter};
use huxley_knowledge::categories::{self, CategoryInput};
use uuid::Uuid;

async fn client(pools: &TestPools) -> Uuid {
    let root = ensure_root(pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let org = huxley_orgs::create(
        &mut tx,
        &ctx,
        root,
        huxley_orgs::OrgKind::Client,
        &unique("KB client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    org
}

fn input(org_id: Uuid, title: &str, body: &str) -> ArticleInput {
    ArticleInput {
        org_id,
        title: title.into(),
        body: Some(body.into()),
        category_id: None,
        visibility: None,
        tags: Some(vec!["MFA".into(), "mfa".into(), " entra ".into()]),
    }
}

#[tokio::test]
async fn an_article_is_written_reviewed_published_and_revised() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();

    let a = articles::create(&mut tx, &ctx, &input(org, "Reset MFA", "1. Open **Entra**"))
        .await
        .unwrap();
    assert_eq!(
        (a.summary.status.as_str(), a.summary.revision),
        ("draft", 1)
    );
    assert_eq!(a.summary.tags, ["mfa", "entra"]);
    assert!(a.body_html.contains("<strong>Entra</strong>"));
    // Nothing is published, so readers see nothing.
    assert!(a.clone().for_reader().is_none());
    // Only drafts are submitted; only articles in review are rejected.
    assert!(matches!(
        articles::reject(&mut tx, &ctx, a.summary.id, "no").await,
        Err(KbError::State(_))
    ));
    let a = articles::submit(&mut tx, &ctx, a.summary.id).await.unwrap();
    assert_eq!(a.summary.status, "in_review");
    assert!(matches!(
        articles::submit(&mut tx, &ctx, a.summary.id).await,
        Err(KbError::State(_))
    ));
    // A rejection needs a reason, and sends it back as a draft.
    assert!(matches!(
        articles::reject(&mut tx, &ctx, a.summary.id, "  ").await,
        Err(KbError::Invalid { .. })
    ));
    let a = articles::reject(&mut tx, &ctx, a.summary.id, "Add the PowerShell way")
        .await
        .unwrap();
    assert_eq!(a.summary.status, "draft");
    assert_eq!(
        a.activity.last().unwrap().details["comment"],
        "Add the PowerShell way"
    );

    // Edited, resubmitted, approved: published at revision 2.
    let a = articles::update(
        &mut tx,
        &ctx,
        a.summary.id,
        &ArticleChanges {
            version: a.version,
            body: Some("1. Open **Entra**\n2. Or run `Reset-Mfa`".into()),
            ..ArticleChanges::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(a.summary.revision, 2);
    articles::submit(&mut tx, &ctx, a.summary.id).await.unwrap();
    let a = articles::publish(&mut tx, &ctx, a.summary.id, Some("Looks good"))
        .await
        .unwrap();
    assert_eq!(
        (a.summary.status.as_str(), a.summary.published_revision),
        ("published", Some(2))
    );
    let kinds: Vec<&str> = a.activity.iter().map(|k| k.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "created",
            "submitted",
            "rejected",
            "edited",
            "submitted",
            "published"
        ]
    );
    assert_eq!(a.activity.last().unwrap().details["reviewed"], true);

    edit_after_publishing(&mut tx, &ctx, a).await;
    tx.commit().await.unwrap();
}

/// Edits, revisions and archiving of a published article.
async fn edit_after_publishing(
    tx: &mut sqlx::PgConnection,
    ctx: &RequestContext,
    a: articles::Article,
) {
    let tx = &mut *tx;
    // Editing a published article is a draft beside it: readers keep revision 2.
    let stale = a.version;
    let a = articles::update(
        tx,
        ctx,
        a.summary.id,
        &ArticleChanges {
            version: a.version,
            title: Some("Reset MFA (new portal)".into()),
            ..ArticleChanges::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (a.summary.status.as_str(), a.summary.revision),
        ("draft", 3)
    );
    let reader = a.clone().for_reader().unwrap();
    assert_eq!(
        (reader.summary.title.as_str(), reader.summary.revision),
        ("Reset MFA", 2)
    );
    assert!(reader.body.contains("Reset-Mfa") && reader.activity.is_empty());
    // A change made from an old read is refused.
    let conflict = articles::update(
        tx,
        ctx,
        a.summary.id,
        &ArticleChanges {
            version: stale,
            title: Some("x".into()),
            ..ArticleChanges::default()
        },
    )
    .await;
    assert!(matches!(conflict, Err(KbError::VersionConflict)));
    // Changing only its tags isn't a new revision, nor a change of status.
    let a = articles::update(
        tx,
        ctx,
        a.summary.id,
        &ArticleChanges {
            version: a.version,
            tags: Some(vec!["identity".into()]),
            ..ArticleChanges::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (a.summary.revision, a.summary.tags.clone()),
        (3, vec!["identity".to_owned()])
    );

    // Every revision is kept.
    let revisions = articles::revisions(tx, a.summary.id).await.unwrap();
    assert_eq!(
        revisions.iter().map(|r| r.number).collect::<Vec<_>>(),
        [3, 2, 1]
    );
    let first = articles::revision(tx, a.summary.id, 1).await.unwrap();
    assert_eq!(first.body.as_deref(), Some("1. Open **Entra**"));

    // Archived: hidden from readers and unchangeable until restored.
    let a = articles::archive(tx, ctx, a.summary.id).await.unwrap();
    assert!(a.clone().for_reader().is_none());
    assert!(matches!(
        articles::update(
            tx,
            ctx,
            a.summary.id,
            &ArticleChanges {
                version: a.version,
                ..ArticleChanges::default()
            }
        )
        .await,
        Err(KbError::State(_))
    ));
    // It had unpublished changes, so it comes back as a draft.
    let a = articles::restore(tx, ctx, a.summary.id).await.unwrap();
    assert_eq!(a.summary.status, "draft");
}

#[tokio::test]
async fn articles_are_searched_and_scoped() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let other = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let cat = categories_here(&pools, org, other).await;
    let word = unique("zebrafirewall").replace(['-', ' '], "");
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut vpn = input(
        org,
        "VPN keeps dropping",
        &format!("Check the {word} rules and the MTU."),
    );
    vpn.category_id = Some(cat.id);
    let vpn = articles::create(&mut tx, &ctx, &vpn).await.unwrap();
    articles::publish(&mut tx, &ctx, vpn.summary.id, None)
        .await
        .unwrap();
    let draft = articles::create(
        &mut tx,
        &ctx,
        &input(
            org,
            "Printer driver",
            &format!("Not {word}-related, a draft"),
        ),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let filter = |drafts| Filter {
        q: Some(word.clone()),
        drafts,
        limit: 50,
        ..Filter::default()
    };
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let (found, total) = articles::list(&mut tx, &filter(Drafts::All)).await.unwrap();
    assert_eq!(total, 2);
    assert!(
        found[0]
            .snippet
            .as_deref()
            .unwrap()
            .contains(&format!("<b>{word}</b>")),
        "{found:?}"
    );
    // Readers find only what's published.
    let (found, _) = articles::list(&mut tx, &filter(Drafts::None))
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|f| f.id).collect::<Vec<_>>(),
        [vpn.summary.id]
    );
    assert_eq!(found[0].category_name.as_deref(), Some("Networking"));
    assert_eq!(categories::get(&mut tx, cat.id).await.unwrap().articles, 1);
    // Being edited, it stays found by readers, as published.
    let vpn = articles::get(&mut tx, vpn.summary.id).await.unwrap();
    articles::update(
        &mut tx,
        &ctx,
        vpn.summary.id,
        &ArticleChanges {
            version: vpn.version,
            title: Some("VPN draft title".into()),
            body: Some("Rewritten without the word".into()),
            ..ArticleChanges::default()
        },
    )
    .await
    .unwrap();
    let (found, _) = articles::list(&mut tx, &filter(Drafts::None))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].title.as_str(), found[0].status.as_str()),
        ("VPN keeps dropping", "published")
    );
    // Where they may write, they see the draft, which no longer matches.
    let (found, _) = articles::list(&mut tx, &filter(Drafts::In(vec![org])))
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|f| f.id).collect::<Vec<_>>(),
        [draft.summary.id]
    );
    tx.commit().await.unwrap();

    // Another client's scope sees none of it.
    let mut tx = begin_scoped(&pools.app, &OrgScope::Orgs(vec![other]))
        .await
        .unwrap();
    assert_eq!(
        articles::list(&mut tx, &filter(Drafts::All))
            .await
            .unwrap()
            .1,
        0
    );
    assert!(matches!(
        articles::get(&mut tx, draft.summary.id).await,
        Err(KbError::NotFound)
    ));
    tx.commit().await.unwrap();
}

/// A category at `org`; names are unique per organization, and another
/// client's category can't be used.
async fn categories_here(pools: &TestPools, org: Uuid, other: Uuid) -> categories::Category {
    let ctx = RequestContext::system(Via::System);
    let named = |name: &str| CategoryInput {
        name: name.into(),
        description: None,
        position: None,
    };
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let cat = categories::create(&mut tx, &ctx, org, &named("Networking"))
        .await
        .unwrap();
    let elsewhere = categories::create(&mut tx, &ctx, other, &named("Other"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // The same name twice at one organization is refused.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    assert!(matches!(
        categories::create(&mut tx, &ctx, org, &named("networking")).await,
        Err(KbError::Invalid { .. })
    ));
    drop(tx);
    // A category of another client can't be used.
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let mut wrong = input(org, "x", "y");
    wrong.category_id = Some(elsewhere.id);
    assert!(matches!(
        articles::create(&mut tx, &ctx, &wrong).await,
        Err(KbError::Invalid { .. })
    ));
    drop(tx);
    cat
}
