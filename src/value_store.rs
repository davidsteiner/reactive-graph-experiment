use std::any::Any;
use std::collections::HashMap;

use crate::node_ref::{NodeId, NodeRef};

/// Typed centralised storage for node values.
/// Each active node has at most one value in the store.
pub struct ValueStore {
    values: HashMap<NodeId, Box<dyn Any + Send + Sync>>,
}

impl ValueStore {
    pub fn new() -> Self {
        ValueStore {
            values: HashMap::new(),
        }
    }

    /// Insert or update a value for the given node.
    pub fn set<T: Send + Sync + 'static>(&mut self, node_ref: &NodeRef<T>, value: T) {
        self.values.insert(node_ref.id.clone(), Box::new(value));
    }

    /// Insert or update a value by NodeId (type-erased).
    pub fn set_by_id(&mut self, id: &NodeId, value: Box<dyn Any + Send + Sync>) {
        self.values.insert(id.clone(), value);
    }

    /// Get a reference to the value for the given node.
    pub fn get<T: 'static>(&self, node_ref: &NodeRef<T>) -> Option<&T> {
        self.values
            .get(&node_ref.id)
            .and_then(|v| v.downcast_ref::<T>())
    }

    /// Get a type-erased reference by NodeId.
    pub fn get_by_id(&self, id: &NodeId) -> Option<&(dyn Any + Send + Sync)> {
        self.values.get(id).map(|v| v.as_ref())
    }

    /// Remove the value for the given node.
    pub fn remove(&mut self, id: &NodeId) {
        self.values.remove(id);
    }

    /// Check if a value exists for the given node.
    pub fn contains(&self, id: &NodeId) -> bool {
        self.values.contains_key(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::marker::PhantomData;

    fn make_ref<T: 'static>(name: &str) -> NodeRef<T> {
        NodeRef {
            id: NodeId::from_key::<_, ()>(name),
            _marker: PhantomData,
        }
    }

    #[test]
    fn test_value_store_typed_access() {
        let mut store = ValueStore::new();

        let float_ref = make_ref::<f64>("test.float");
        let string_ref = make_ref::<String>("test.string");

        store.set(&float_ref, 42.0);
        store.set(&string_ref, "hello".to_string());

        assert_eq!(store.get(&float_ref), Some(&42.0));
        assert_eq!(store.get(&string_ref), Some(&"hello".to_string()));

        // Update
        store.set(&float_ref, 99.0);
        assert_eq!(store.get(&float_ref), Some(&99.0));

        // Remove
        store.remove(&float_ref.id);
        assert_eq!(store.get(&float_ref), None);
    }
}
