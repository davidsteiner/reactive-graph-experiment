use std::any::TypeId;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// Unique identity for a node in the graph.
/// TypeId scopes keys by concrete node type so different node types never collide.
#[derive(Clone)]
pub struct NodeId {
    pub(crate) type_id: TypeId,
    pub(crate) key_hash: u64,
    pub(crate) key_debug: String,
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.key_debug)
    }
}

impl PartialEq for NodeId {
    fn eq(&self, other: &Self) -> bool {
        self.type_id == other.type_id && self.key_hash == other.key_hash
    }
}

impl Eq for NodeId {}

impl Hash for NodeId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.type_id.hash(state);
        self.key_hash.hash(state);
    }
}

impl NodeId {
    /// Create a NodeId from a concrete node type's key.
    pub fn from_key<K: Hash + Eq + fmt::Debug + 'static + ?Sized, Scope: 'static>(key: &K) -> Self {
        use std::hash::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let key_hash = hasher.finish();
        NodeId {
            type_id: TypeId::of::<Scope>(),
            key_hash,
            key_debug: format!("{:?}", key),
        }
    }
}

/// A typed, lightweight reference to a node in the graph.
/// Carries the output type at compile time. Copy + Clone + Send + Sync.
pub struct NodeRef<T> {
    pub(crate) id: NodeId,
    pub(crate) _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for NodeRef<T> {
    fn clone(&self) -> Self {
        NodeRef {
            id: self.id.clone(),
            _marker: PhantomData,
        }
    }
}

// NodeRef is not Copy because NodeId contains String.
// It is Clone, Send, and Sync.

impl<T> fmt::Debug for NodeRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeRef({:?})", self.id)
    }
}

// NodeRef is Send + Sync because it contains only NodeId (Send+Sync) and PhantomData<fn() -> T> (Send+Sync for all T).
