//! The `knowledge` module's steps (`core.kb.*`).

use huxley_db::{OrgScope, begin_scoped};
use huxley_knowledge::embeddings;
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

impl Engine {
    /// `core.kb.embed`: embeds stale published articles at `org_id` and
    /// below, a batch per organization by its embeddings provider. Nothing
    /// to do without pgvector. A provider that fails is reported, not fatal.
    pub(crate) async fn embed_articles(&self, org_id: Uuid, input: &Json) -> Result<Json, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let active = huxley_modules::is_active_at(&mut tx, huxley_knowledge::MODULE, org_id)
            .await
            .map_err(internal)?;
        if !active {
            return Err(error(
                "module_disabled",
                "The knowledge base (the knowledge module) is turned off for this organization",
            ));
        }
        let limit = input["limit"].as_i64().map_or(200, |n| n.clamp(1, 2000));
        if !embeddings::available(&mut tx).await.map_err(internal)? {
            tx.commit().await.map_err(internal)?;
            return Ok(json!({ "available": false, "embedded": 0, "errors": [] }));
        }
        let stale = embeddings::stale(&mut tx, org_id, limit)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        let mut batches: Vec<(Uuid, Vec<embeddings::Stale>)> = Vec::new();
        for article in stale {
            match batches.iter_mut().find(|(org, _)| *org == article.org_id) {
                Some((_, articles)) => articles.push(article),
                None => batches.push((article.org_id, vec![article])),
            }
        }
        let ai = self.ai();
        let purpose = huxley_ai::Purpose {
            feature: "kb_embeddings",
            run_id: None,
        };
        let (mut embedded, mut errors) = (0_u64, Vec::new());
        for (org, articles) in batches {
            for chunk in articles.chunks(huxley_ai::embed::MAX_BATCH) {
                let texts: Vec<String> = chunk.iter().map(|a| a.text.clone()).collect();
                let done = match ai
                    .embed(org, purpose, &texts, huxley_ai::embed::InputType::Document)
                    .await
                {
                    Ok(done) => done,
                    Err(huxley_ai::AiError::Internal(e)) => return Err(internal(e)),
                    Err(e) => {
                        errors.push(json!({ "org_id": org, "error": e.to_string() }));
                        break;
                    }
                };
                let mut tx = begin_scoped(self.db(), &OrgScope::System)
                    .await
                    .map_err(internal)?;
                for (article, vector) in chunk.iter().zip(&done.vectors) {
                    embeddings::save(&mut tx, article, vector)
                        .await
                        .map_err(internal)?;
                    embedded += 1;
                }
                tx.commit().await.map_err(internal)?;
            }
        }
        Ok(json!({ "available": true, "embedded": embedded, "errors": errors }))
    }
}
