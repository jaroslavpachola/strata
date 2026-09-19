//! The store's operations as a trait, so a front end can hold the files
//! or a client of a running strata-server and not care which.

use std::path::PathBuf;

use uuid::Uuid;

use crate::Result;
use crate::model::{Entry, Item, Partition, PropertyDef, TypeDef, Values, VaultStatus};
use crate::query::Query;
use crate::seed::Seeded;
use crate::store::{Relation, Store};
use crate::transfer::{Imported, TypeExport};

/// Everything a front end does with a store. [`Store`] is the files;
/// strata-server's client is the same operations over its socket.
pub trait Api {
    /// The store's directory, if it has one.
    fn dir(&self) -> Option<PathBuf>;

    fn vault_status(&self) -> Result<VaultStatus>;
    fn vault_create(&mut self, passphrase: &str) -> Result<()>;
    fn vault_unlock(&mut self, passphrase: &str) -> Result<()>;
    fn vault_lock(&mut self) -> Result<()>;

    fn add_type(&self, def: &TypeDef) -> Result<()>;
    fn list_types(&self) -> Result<Vec<TypeDef>>;
    fn get_type(&self, name: &str) -> Result<TypeDef>;
    fn set_description(&self, type_name: &str, description: &str) -> Result<()>;
    fn add_property(&self, type_name: &str, property: &PropertyDef) -> Result<()>;
    fn remove_property(&self, type_name: &str, property: &str) -> Result<()>;
    fn rename_property(&self, type_name: &str, from: &str, to: &str) -> Result<()>;
    fn set_required(&self, type_name: &str, property: &str, required: bool) -> Result<()>;
    fn set_choices(
        &self,
        type_name: &str,
        property: &str,
        choices: Option<Vec<String>>,
    ) -> Result<()>;
    fn move_type(&self, type_name: &str, to: Partition) -> Result<()>;

    fn add_items(&self, type_name: &str, bodies: Vec<Values>, author: &str) -> Result<Vec<Item>>;
    fn get_item(&self, id: Uuid) -> Result<Item>;
    fn get(&self, id: Uuid) -> Result<Entry>;
    fn update_item(&self, id: Uuid, patch: Values, author: &str) -> Result<Item>;
    fn delete_item(&self, id: Uuid) -> Result<()>;
    fn relate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()>;
    fn unrelate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()>;
    fn relations(&self, id: Uuid) -> Result<Vec<Relation>>;
    fn query(&self, q: &Query) -> Result<Vec<Entry>>;

    fn seed(&self) -> Result<Seeded>;
    fn seed_needs_vault(&self) -> Result<bool>;

    fn export_type(&self, type_name: &str) -> Result<TypeExport>;
    fn import(&self, types: &[TypeExport]) -> Result<Imported>;

    fn add_item(&self, type_name: &str, values: Values, author: &str) -> Result<Item> {
        let mut items = self.add_items(type_name, vec![values], author)?;
        Ok(items.remove(0))
    }
}

impl Api for Store {
    fn dir(&self) -> Option<PathBuf> {
        Store::dir(self).map(PathBuf::from)
    }
    fn vault_status(&self) -> Result<VaultStatus> {
        Ok(Store::vault_status(self))
    }
    fn vault_create(&mut self, passphrase: &str) -> Result<()> {
        Store::vault_create(self, passphrase)
    }
    fn vault_unlock(&mut self, passphrase: &str) -> Result<()> {
        Store::vault_unlock(self, passphrase)
    }
    fn vault_lock(&mut self) -> Result<()> {
        Store::vault_lock(self)
    }
    fn add_type(&self, def: &TypeDef) -> Result<()> {
        Store::add_type(self, def)
    }
    fn list_types(&self) -> Result<Vec<TypeDef>> {
        Store::list_types(self)
    }
    fn get_type(&self, name: &str) -> Result<TypeDef> {
        Store::get_type(self, name)
    }
    fn set_description(&self, type_name: &str, description: &str) -> Result<()> {
        Store::set_description(self, type_name, description)
    }
    fn add_property(&self, type_name: &str, property: &PropertyDef) -> Result<()> {
        Store::add_property(self, type_name, property)
    }
    fn remove_property(&self, type_name: &str, property: &str) -> Result<()> {
        Store::remove_property(self, type_name, property)
    }
    fn rename_property(&self, type_name: &str, from: &str, to: &str) -> Result<()> {
        Store::rename_property(self, type_name, from, to)
    }
    fn set_required(&self, type_name: &str, property: &str, required: bool) -> Result<()> {
        Store::set_required(self, type_name, property, required)
    }
    fn set_choices(
        &self,
        type_name: &str,
        property: &str,
        choices: Option<Vec<String>>,
    ) -> Result<()> {
        Store::set_choices(self, type_name, property, choices)
    }
    fn move_type(&self, type_name: &str, to: Partition) -> Result<()> {
        Store::move_type(self, type_name, to)
    }
    fn add_items(&self, type_name: &str, bodies: Vec<Values>, author: &str) -> Result<Vec<Item>> {
        Store::add_items(self, type_name, bodies, author)
    }
    fn get_item(&self, id: Uuid) -> Result<Item> {
        Store::get_item(self, id)
    }
    fn get(&self, id: Uuid) -> Result<Entry> {
        Store::get(self, id)
    }
    fn update_item(&self, id: Uuid, patch: Values, author: &str) -> Result<Item> {
        Store::update_item(self, id, patch, author)
    }
    fn delete_item(&self, id: Uuid) -> Result<()> {
        Store::delete_item(self, id)
    }
    fn relate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        Store::relate(self, source, target, kind)
    }
    fn unrelate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        Store::unrelate(self, source, target, kind)
    }
    fn relations(&self, id: Uuid) -> Result<Vec<Relation>> {
        Store::relations(self, id)
    }
    fn query(&self, q: &Query) -> Result<Vec<Entry>> {
        Store::query(self, q)
    }
    fn seed(&self) -> Result<Seeded> {
        Store::seed(self)
    }
    fn seed_needs_vault(&self) -> Result<bool> {
        Store::seed_needs_vault(self)
    }
    fn export_type(&self, type_name: &str) -> Result<TypeExport> {
        Store::export_type(self, type_name)
    }
    fn import(&self, types: &[TypeExport]) -> Result<Imported> {
        Store::import(self, types)
    }
}
