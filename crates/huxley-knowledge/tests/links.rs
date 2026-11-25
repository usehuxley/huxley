//! Articles linked to tickets, and suggested for them, against a real
//! PostgreSQL.

use huxley_core::{RequestContext, Via};
use huxley_db::testing::{TestPools, ensure_root, pools, unique};
use huxley_db::{OrgScope, begin_scoped};
use huxley_knowledge::KbError;
use huxley_knowledge::articles::{self, ArticleInput, Drafts};
use huxley_knowledge::links;
use huxley_servicedesk::tickets::{self, NewTicket};
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
        &unique("Links client"),
    )
    .await
    .unwrap()
    .id;
    tx.commit().await.unwrap();
    org
}

async fn article(
    tx: &mut sqlx::PgConnection,
    org: Uuid,
    title: &str,
    body: &str,
    publish: bool,
) -> Uuid {
    let ctx = RequestContext::system(Via::System);
    let input = ArticleInput {
        org_id: org,
        title: title.into(),
        body: Some(body.into()),
        category_id: None,
        visibility: None,
        tags: None,
    };
    let a = articles::create(&mut *tx, &ctx, &input).await.unwrap();
    if publish {
        articles::publish(&mut *tx, &ctx, a.summary.id, None)
            .await
            .unwrap();
    }
    a.summary.id
}

#[tokio::test]
async fn articles_are_linked_and_suggested() {
    let Some(pools) = pools().await else { return };
    let org = client(&pools).await;
    let ctx = RequestContext::system(Via::System);
    let word = unique("quokkaprinter").replace(['-', ' '], "");
    let mut tx = begin_scoped(&pools.app, &OrgScope::System).await.unwrap();
    let ticket = tickets::create(
        &mut tx,
        &ctx,
        &NewTicket {
            org_id: org,
            title: format!("The {word} won't print"),
            description: Some(format!("Our {word} jams on every page")),
            ..NewTicket::default()
        },
        "T",
        None,
    )
    .await
    .unwrap();
    let fix = article(
        &mut tx,
        org,
        &format!("Clearing {word} jams"),
        &format!("Open the {word} tray."),
        true,
    )
    .await;
    let other = article(&mut tx, org, "VPN setup", "Install the client.", true).await;
    let draft = article(
        &mut tx,
        org,
        &format!("{word} drivers"),
        &format!("{word} driver notes"),
        false,
    )
    .await;
    let text = format!("{} {}", ticket.title, ticket.description);

    // The matching published article is suggested; drafts and unrelated ones aren't.
    let suggested = links::suggest(&mut tx, ticket.id, &text, 5).await.unwrap();
    // (The shared test database may hold other matching articles; this one ranks first.)
    let ids: Vec<Uuid> = suggested.iter().map(|s| s.id).collect();
    assert_eq!(ids.first(), Some(&fix), "{suggested:?}");
    assert!(!ids.contains(&other) && !ids.contains(&draft));
    assert!(suggested[0].snippet.as_deref().unwrap().contains("<b>"));

    // Linked: listed, no longer suggested; linking twice is fine.
    links::link(&mut tx, &ctx, fix, ticket.id).await.unwrap();
    links::link(&mut tx, &ctx, fix, ticket.id).await.unwrap();
    links::link(&mut tx, &ctx, draft, ticket.id).await.unwrap();
    let after = links::suggest(&mut tx, ticket.id, &text, 5).await.unwrap();
    assert!(after.iter().all(|s| s.id != fix && s.id != draft));
    let linked = links::for_ticket(&mut tx, ticket.id, &Drafts::All)
        .await
        .unwrap();
    assert_eq!(
        linked.iter().map(|l| l.id).collect::<Vec<_>>(),
        [fix, draft]
    );
    // Readers don't see the linked draft.
    let linked = links::for_ticket(&mut tx, ticket.id, &Drafts::None)
        .await
        .unwrap();
    assert_eq!(linked.iter().map(|l| l.id).collect::<Vec<_>>(), [fix]);
    let used = links::tickets_for_article(&mut tx, fix).await.unwrap();
    assert_eq!(used[0].reference, ticket.reference);

    // Unlinked; unlinking what isn't linked is not found; archived can't be linked.
    links::unlink(&mut tx, &ctx, fix, ticket.id).await.unwrap();
    assert!(matches!(
        links::unlink(&mut tx, &ctx, fix, ticket.id).await,
        Err(KbError::NotFound)
    ));
    articles::archive(&mut tx, &ctx, other).await.unwrap();
    assert!(matches!(
        links::link(&mut tx, &ctx, other, ticket.id).await,
        Err(KbError::State(_))
    ));
    tx.commit().await.unwrap();
}
