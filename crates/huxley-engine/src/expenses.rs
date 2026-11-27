//! Expense steps (ADR-080): reading a receipt with AI.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use huxley_ai::{Attachment, Purpose, Request};
use huxley_db::{OrgScope, begin_scoped};
use huxley_expenses::entries::{self, Reading};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::triage::{id, internal};
use crate::walk::error;

/// The largest receipt sent to the AI provider.
const MAX_RECEIPT_BYTES: usize = 10 * 1024 * 1024;

const SYSTEM: &str = "You read receipts and invoices for expense claims. Give the total paid (tax included) and \
    the tax as plain decimal numbers without currency symbols, the ISO 4217 currency code, the date of purchase \
    as YYYY-MM-DD, the merchant's name, and a few words on what was bought. Use null for anything you can't \
    read with confidence; never guess.";

/// The answer's shape: every field a string or null.
fn schema() -> Json {
    let field = json!({ "type": ["string", "null"] });
    json!({
        "type": "object",
        "properties": {
            "amount": field, "tax": field, "currency": field, "spentOn": field, "vendor": field, "description": field
        },
        "required": ["amount", "tax", "currency", "spentOn", "vendor", "description"],
        "additionalProperties": false
    })
}

impl Engine {
    /// `core.expense.read_receipt`: has AI read an expense's receipt and
    /// keeps what it read beside the entry, for the person to confirm.
    /// Output: the reading.
    pub(crate) async fn read_receipt(
        &self,
        org_id: Uuid,
        run_id: Uuid,
        input: &Json,
    ) -> Result<Json, Json> {
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let on = huxley_modules::is_active_at(&mut tx, huxley_expenses::MODULE, org_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if !on {
            return Err(error("module_disabled", "Expenses are turned off"));
        }
        let expense =
            id(input, "expense_id")?.ok_or_else(|| error("input", "expense_id is required"))?;
        let storage = self
            .payloads
            .storage()
            .ok_or_else(|| error("unavailable", "receipts need object storage"))?
            .clone();
        let subtree = self.subtree(org_id).await?;
        let scope = OrgScope::Orgs(subtree);
        let mut tx = begin_scoped(self.db(), &scope).await.map_err(internal)?;
        let found = entries::receipt_for_reading(&mut tx, &storage, expense)
            .await
            .map_err(|e| match e {
                huxley_expenses::ExpenseError::NotFound => {
                    error("not_found", "no such expense here")
                }
                other => internal(other),
            })?;
        tx.commit().await.map_err(internal)?;
        let Some((media_type, data)) = found else {
            return Err(error("input", "the expense has no receipt"));
        };
        if data.len() > MAX_RECEIPT_BYTES {
            return Err(error("input", "the receipt is too large to read"));
        }
        let request = Request {
            system: Some(SYSTEM.to_owned()),
            prompt: "Read this receipt.".to_owned(),
            schema: Some(schema()),
            max_tokens: 0,
            attachment: Some(Attachment {
                media_type,
                base64: STANDARD.encode(&data),
            }),
        };
        let purpose = Purpose {
            feature: "expenses",
            run_id: Some(run_id),
        };
        let read = self
            .ai()
            .complete(org_id, purpose, input["model"].as_str(), &request)
            .await;
        let mut tx = begin_scoped(self.db(), &scope).await.map_err(internal)?;
        let outcome = match &read {
            Ok(done) => {
                let reading: Reading = done
                    .data
                    .clone()
                    .and_then(|d| serde_json::from_value(d).ok())
                    .unwrap_or_default();
                entries::record_reading(&mut tx, expense, Ok(&reading))
                    .await
                    .map_err(internal)?;
                Ok(json!(reading))
            }
            Err(e) => {
                entries::record_reading(&mut tx, expense, Err(&e.to_string()))
                    .await
                    .map_err(internal)?;
                Err(error("ai", e.to_string()))
            }
        };
        tx.commit().await.map_err(internal)?;
        outcome
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_field_is_required_and_nullable() {
        let s = super::schema();
        assert_eq!(s["required"].as_array().unwrap().len(), 6);
        assert_eq!(s["properties"]["vendor"]["type"][1], "null");
    }
}
