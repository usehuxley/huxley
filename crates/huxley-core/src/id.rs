//! Identifiers.
//!
//! All primary keys are UUIDv7: globally unique and time-sortable, which keeps
//! B-tree indexes compact and lets cursor pagination order by id.

use uuid::Uuid;

/// Generates a new UUIDv7 identifier.
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_v7_and_sortable() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.get_version_num(), 7);
        assert!(
            a < b,
            "UUIDv7 values generated in sequence must sort in order"
        );
    }
}
