//! The types every store starts with.

use serde::Serialize;

use crate::model::{Kind, Partition, PropertyDef, TypeDef};
use crate::{Error, Result, Store};

/// Task in the open, PortfolioSnapshot in the vault.
pub fn seed_types() -> Vec<TypeDef> {
    vec![
        TypeDef::new("Task")
            .description("Something to do. note is a SuperHub vault path, e.g. Projects/strata.md")
            .property(PropertyDef::new("title", Kind::Text).required())
            .property(
                PropertyDef::new("status", Kind::Text)
                    .required()
                    .choices(["todo", "doing", "done"]),
            )
            .property(PropertyDef::new("due", Kind::Date))
            .property(PropertyDef::new("project", Kind::Text))
            .property(PropertyDef::new("note", Kind::Text)),
        TypeDef::new("PortfolioSnapshot")
            .partition(Partition::Vault)
            .description(
                "A portfolio's value at one moment. positions: [{symbol, quantity, price, value}]",
            )
            .property(PropertyDef::new("taken_at", Kind::Date).required())
            .property(PropertyDef::new("currency", Kind::Text).required())
            .property(PropertyDef::new("total", Kind::Number).required())
            .property(PropertyDef::new("positions", Kind::Json).required()),
    ]
}

/// What [`Store::seed`] did.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Seeded {
    /// Declared now.
    pub added: Vec<String>,
    /// Missing, but in the vault, which is locked or absent.
    pub skipped: Vec<String>,
}

impl Store {
    /// Declare the seed types the store lacks. One that exists is left as
    /// it is, whatever it has become since.
    pub fn seed(&self) -> Result<Seeded> {
        let mut done = Seeded::default();
        for def in seed_types() {
            match self.get_type(&def.name) {
                Ok(_) => continue,
                Err(Error::UnknownType(_)) => {}
                Err(e) => return Err(e),
            }
            match self.add_type(&def) {
                Ok(()) => done.added.push(def.name),
                Err(Error::VaultLocked | Error::NoVault) => done.skipped.push(def.name),
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    /// Whether [`Store::seed`] would need the vault unlocked to finish.
    pub fn seed_needs_vault(&self) -> Result<bool> {
        for def in seed_types() {
            if def.partition == Partition::Vault {
                match self.get_type(&def.name) {
                    Ok(_) => {}
                    Err(Error::UnknownType(_)) => return Ok(true),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_is_idempotent_and_waits_for_the_vault() {
        let mut store = Store::open_in_memory().unwrap();
        let first = store.seed().unwrap();
        assert_eq!(first.added, ["Task"]);
        assert_eq!(first.skipped, ["PortfolioSnapshot"]);
        assert!(store.seed_needs_vault().unwrap());

        store.vault_create("pw").unwrap();
        let second = store.seed().unwrap();
        assert_eq!(second.added, ["PortfolioSnapshot"]);
        assert!(second.skipped.is_empty());
        assert!(!store.seed_needs_vault().unwrap());

        assert_eq!(store.seed().unwrap(), Seeded::default());
        for def in seed_types() {
            assert_eq!(store.get_type(&def.name).unwrap(), def);
        }
    }

    #[test]
    fn a_changed_seed_type_is_left_alone() {
        let store = Store::open_in_memory().unwrap();
        store.seed().unwrap();
        store.set_choices("Task", "status", None).unwrap();
        store.seed().unwrap();
        let status = store.get_type("Task").unwrap();
        assert_eq!(status.get("status").unwrap().choices, None);
    }
}
