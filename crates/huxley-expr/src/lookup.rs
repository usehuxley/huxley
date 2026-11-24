//! `lookup(dataset, column=value, …)`: one row of a dataset, in a workflow
//! run (design doc §9).
//!
//! Expressions are evaluated synchronously and replayed every time a run
//! resumes, so a lookup can't query the database as it's evaluated: a row
//! that changed between replays would send the run down a different path.
//! Instead each run **records** every lookup's answer the first time it asks,
//! and replays use the recorded answer:
//!
//! - An answer in [`Lookups`] is returned (the row, or `none`).
//! - An unknown one is noted as missing and evaluation stops. The engine
//!   fetches the missing ones, records them, and evaluates again; a step
//!   waiting on a lookup isn't a failure.
//!
//! Outside runs (form previews, checks) `lookup` is a global that explains
//! it isn't available there.

use std::collections::BTreeMap;
use std::sync::Mutex;

use minijinja::value::Kwargs;
use minijinja::{Error, ErrorKind, Value};
use serde_json::{Value as Json, json};

/// Most distinct lookups one evaluation round may ask for.
pub const MAX_MISSING: usize = 50;

/// What a lookup asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupRequest {
    pub dataset: String,
    /// Column values the row must equal.
    pub filters: BTreeMap<String, Json>,
}

impl LookupRequest {
    /// A stable key for the request (filters are sorted by column).
    pub fn key(&self) -> String {
        json!({ "dataset": self.dataset, "filters": self.filters }).to_string()
    }
}

/// A recorded answer: the matching row (`None` for no match), or why the
/// lookup failed (an unknown dataset, say).
pub type LookupAnswer = Result<Option<Json>, String>;

/// A run's recorded lookup answers, and the ones it still needs.
#[derive(Debug, Default)]
pub struct Lookups {
    known: BTreeMap<String, LookupAnswer>,
    missing: Mutex<BTreeMap<String, LookupRequest>>,
}

impl Lookups {
    pub fn new(known: BTreeMap<String, LookupAnswer>) -> Self {
        Self {
            known,
            missing: Mutex::default(),
        }
    }

    /// How many lookups are waiting for an answer.
    pub fn missing_count(&self) -> usize {
        self.missing().len()
    }

    /// The lookups waiting for an answer.
    pub fn take_missing(&self) -> Vec<LookupRequest> {
        std::mem::take(&mut *self.missing()).into_values().collect()
    }

    fn missing(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, LookupRequest>> {
        self.missing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn call(&self, dataset: &str, kwargs: &Kwargs) -> Result<Value, Error> {
        let request = request(dataset, kwargs)?;
        let key = request.key();
        match self.known.get(&key) {
            Some(Ok(Some(row))) => Ok(Value::from(minijinja::value::Serde(row))),
            Some(Ok(None)) => Ok(Value::from(())),
            Some(Err(why)) => Err(invalid(format!("lookup({dataset:?}): {why}"))),
            None => {
                let mut missing = self.missing();
                if missing.len() >= MAX_MISSING && !missing.contains_key(&key) {
                    return Err(invalid(format!(
                        "more than {MAX_MISSING} different lookups at once"
                    )));
                }
                missing.insert(key, request);
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "waiting for a lookup",
                ))
            }
        }
    }
}

fn invalid(message: String) -> Error {
    Error::new(ErrorKind::InvalidOperation, message)
}

fn request(dataset: &str, kwargs: &Kwargs) -> Result<LookupRequest, Error> {
    if dataset.trim().is_empty() {
        return Err(invalid("lookup() needs a dataset key".into()));
    }
    let mut filters = BTreeMap::new();
    for column in kwargs.args() {
        let value: Value = kwargs.get(column)?;
        let value = serde_json::to_value(&value)
            .map_err(|e| invalid(format!("lookup(): {column}: {e}")))?;
        if value.is_object() || value.is_array() {
            return Err(invalid(format!(
                "lookup(): {column} must be a text, number, boolean or none"
            )));
        }
        filters.insert(column.to_owned(), value);
    }
    if filters.is_empty() {
        return Err(invalid(
            "lookup() needs at least one column to match, e.g. lookup('users', upn=inputs.email)"
                .into(),
        ));
    }
    Ok(LookupRequest {
        dataset: dataset.to_owned(),
        filters,
    })
}

/// The global used where there's no run to record lookups in.
pub(crate) fn unavailable(_dataset: Value, _kwargs: Kwargs) -> Result<Value, Error> {
    Err(invalid(
        "lookup() is available only while a workflow runs".into(),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Engine, Scope};

    fn request(dataset: &str, filters: Json) -> LookupRequest {
        LookupRequest {
            dataset: dataset.into(),
            filters: serde_json::from_value(filters).unwrap(),
        }
    }

    #[test]
    fn answers_come_from_the_recorded_lookups() {
        let engine = Engine::new();
        let ada = request("users", json!({ "upn": "ada@x.test" }));
        let nobody = request("users", json!({ "upn": "nobody@x.test", "seats": 2 }));
        let broken = request("nope", json!({ "id": 1 }));
        let lookups = Arc::new(Lookups::new(BTreeMap::from([
            (
                ada.key(),
                Ok(Some(json!({ "upn": "ada@x.test", "name": "Ada" }))),
            ),
            (nobody.key(), Ok(None)),
            (broken.key(), Err("there's no dataset \"nope\"".into())),
        ])));
        let scope = Scope::new()
            .with("inputs", &json!({ "email": "ada@x.test" }))
            .with_lookups(lookups.clone());

        let name = engine
            .eval_str("{{ lookup('users', upn=inputs.email).name }}", &scope)
            .unwrap();
        assert_eq!(name.value, json!("Ada"));
        assert!(!name.tainted);
        // Keyword order doesn't matter; no match is none.
        let none = engine
            .eval_str(
                "{{ lookup('users', seats=2, upn='nobody@x.test') is none }}",
                &scope,
            )
            .unwrap();
        assert_eq!(none.value, json!(true));
        let failed = engine
            .eval_str("{{ lookup('nope', id=1) }}", &scope)
            .unwrap_err();
        assert!(failed.to_string().contains("no dataset"), "{failed}");
        assert_eq!(lookups.missing_count(), 0);
    }

    #[test]
    fn an_unknown_lookup_is_noted_and_stops_evaluation() {
        let engine = Engine::new();
        let lookups = Arc::new(Lookups::default());
        let scope = Scope::new().with_lookups(lookups.clone());
        assert!(
            engine
                .eval_condition(
                    "{{ lookup('users', upn='a') and lookup('users', upn='b') }}",
                    &scope
                )
                .is_err()
        );
        // Evaluation stopped at the first one.
        assert_eq!(
            lookups.take_missing(),
            vec![request("users", json!({ "upn": "a" }))]
        );
        assert_eq!(lookups.missing_count(), 0);
    }

    #[test]
    fn arguments_are_checked() {
        let engine = Engine::new();
        let lookups = Arc::new(Lookups::default());
        let scope = Scope::new().with_lookups(lookups.clone());
        for (text, why) in [
            ("{{ lookup('users') }}", "at least one column"),
            ("{{ lookup('', upn='a') }}", "dataset key"),
            ("{{ lookup('users', upn=[1]) }}", "must be a text"),
        ] {
            let e = engine.eval_str(text, &scope).unwrap_err();
            assert!(e.to_string().contains(why), "{text}: {e}");
        }
        assert_eq!(lookups.missing_count(), 0);
    }

    #[test]
    fn outside_a_run_it_says_so() {
        let engine = Engine::new();
        assert!(engine.is_global("lookup"));
        let e = engine
            .eval_str("{{ lookup('users', upn='a') }}", &Scope::new())
            .unwrap_err();
        assert!(e.to_string().contains("only while a workflow runs"), "{e}");
    }
}
