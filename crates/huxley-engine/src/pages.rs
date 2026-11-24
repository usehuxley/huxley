//! `all_pages` on `integration.action` steps: following a list action's
//! pages (its pagination comes from the integration's overlay) and
//! collecting their items. A next-page URL must stay on the connection's
//! own origin, so its credentials never go anywhere else.

use huxley_integrations::Pagination;
use serde_json::{Value as Json, json};

use crate::egress::Egress;
use crate::walk::error;

fn at_path<'a>(value: &'a Json, path: Option<&str>) -> Option<&'a Json> {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Some(value);
    };
    // A key may itself contain dots (`@odata.nextLink`): try it whole first.
    if let Some(found) = value.get(path) {
        return Some(found);
    }
    path.split('.').try_fold(value, |v, key| match v {
        Json::Array(items) => items.get(key.parse::<usize>().ok()?),
        _ => v.get(key),
    })
}

fn origin(url: &str) -> Option<(String, String, u16)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    Some((
        parsed.scheme().to_owned(),
        parsed.host_str()?.to_ascii_lowercase(),
        parsed.port_or_known_default()?,
    ))
}

/// The `rel="next"` URL of a `Link` header.
fn link_next(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        let is_next = params.split(';').any(|p| {
            p.trim()
                .replace(' ', "")
                .eq_ignore_ascii_case("rel=\"next\"")
                || p.trim().eq_ignore_ascii_case("rel=next")
        });
        is_next.then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_owned()
        })
    })
}

/// Follows up to `max_pages` pages from the first request `spec`.
pub(crate) async fn all_pages(
    egress: &Egress,
    mut spec: Json,
    base_url: &str,
    pagination: &Pagination,
    max_pages: u64,
    rate_limit: Option<(
        sqlx::PgPool,
        uuid::Uuid,
        huxley_integrations::overlay::RateLimit,
    )>,
) -> Result<Json, Json> {
    let home = origin(base_url);
    let mut items: Vec<Json> = Vec::new();
    let mut pages = 0;
    let mut offset: u64 = 0;
    if let Pagination::Offset {
        page_size,
        limit_param: Some(limit),
        ..
    } = pagination
    {
        spec["query"][limit] = json!(page_size);
    }
    loop {
        // Every page after the first counts against the rate limit (the
        // first was counted before the call); wait for room, briefly.
        if pages > 0
            && let Some((db, connection, limit)) = &rate_limit
        {
            wait_for_room(db, *connection, *limit).await?;
        }
        let response = crate::http::request(egress, &spec).await?;
        pages += 1;
        let body = &response["body"];
        let page = at_path(body, pagination.items())
            .and_then(Json::as_array)
            .ok_or_else(|| {
                error(
                    "pagination",
                    format!(
                        "page {pages} has no list at {:?}",
                        pagination.items().unwrap_or("(the body)")
                    ),
                )
            })?;
        let count = page.len();
        items.extend(page.iter().cloned());
        let next = match pagination {
            Pagination::NextUrl { next, .. } => at_path(body, Some(next))
                .and_then(Json::as_str)
                .map(str::to_owned)
                .map(Next::Url),
            Pagination::LinkHeader { .. } => response["headers"]["link"]
                .as_str()
                .and_then(link_next)
                .map(Next::Url),
            Pagination::Cursor { cursor, param, .. } => at_path(body, Some(cursor))
                .and_then(Json::as_str)
                .filter(|c| !c.is_empty())
                .map(|c| Next::Query(param.clone(), json!(c))),
            Pagination::Offset {
                param, page_size, ..
            } => {
                offset += u64::from(*page_size);
                (count >= *page_size as usize && count > 0)
                    .then(|| Next::Query(param.clone(), json!(offset)))
            }
        };
        let Some(next) = next else {
            return Ok(json!({ "items": items, "pages": pages, "truncated": false }));
        };
        if pages >= max_pages {
            return Ok(json!({ "items": items, "pages": pages, "truncated": true }));
        }
        match next {
            Next::Url(url) => {
                if origin(&url).is_none() || origin(&url) != home {
                    return Err(error(
                        "pagination",
                        format!("the next page {url:?} isn't on the connection's server"),
                    ));
                }
                spec["url"] = json!(url);
                spec["query"] = json!({});
            }
            Next::Query(param, value) => {
                spec["query"][param] = value;
            }
        }
    }
}

/// Waits for the rate limit's next window, at most a minute in all.
async fn wait_for_room(
    db: &sqlx::PgPool,
    connection: uuid::Uuid,
    limit: huxley_integrations::overlay::RateLimit,
) -> Result<(), Json> {
    let give_up = time::OffsetDateTime::now_utc() + time::Duration::seconds(60);
    loop {
        match crate::rate::acquire(db, connection, limit).await {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(at)) if at <= give_up => {
                let wait =
                    (at - time::OffsetDateTime::now_utc()).max(time::Duration::milliseconds(10));
                tokio::time::sleep(wait.try_into().unwrap_or_default()).await;
            }
            Ok(Err(at)) => return Err(crate::action::rate_limited(at)),
            Err(e) => return Err(error("internal", e.to_string())),
        }
    }
}

enum Next {
    Url(String),
    Query(String, Json),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_headers_and_paths() {
        assert_eq!(
            link_next(
                r#"<https://api.x/items?page=2>; rel="next", <https://api.x/items?page=9>; rel="last""#
            ),
            Some("https://api.x/items?page=2".to_owned())
        );
        assert_eq!(link_next(r#"<https://api.x/a>; rel="prev""#), None);
        let body = json!({ "@odata.nextLink": "u", "data": { "list": [1] } });
        assert_eq!(at_path(&body, Some("@odata.nextLink")), Some(&json!("u")));
        assert_eq!(at_path(&body, Some("data.list")), Some(&json!([1])));
        assert_eq!(
            origin("https://a.example/x"),
            origin("https://A.example:443/y")
        );
        assert_ne!(
            origin("https://a.example/x"),
            origin("https://a.example.evil.com/x")
        );
    }
}
