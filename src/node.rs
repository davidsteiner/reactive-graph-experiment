use std::fmt::Debug;
use std::hash::Hash;

/// Base trait for all nodes. Provides identity (key) and output type.
pub trait Node {
    type Output: Send + Sync + 'static;
    type Key: Hash + Eq + Debug + Send + 'static;
    fn key(&self) -> Self::Key;
}

/// A source node — produces values from outside the graph.
pub trait Source: Node {
    type Guard: Send + 'static;
    fn activate(&self, tx: crate::sender::Sender<Self::Output>) -> Self::Guard;
}

/// A synchronous compute node — transforms inputs into an output.
pub trait Compute: Node {
    type Inputs: crate::input_tuple::InputTuple;
    fn compute(
        &self,
        inputs: <Self::Inputs as crate::input_tuple::InputTuple>::Ref<'_>,
    ) -> Self::Output;
}

/// A sink node — consumes values, produces no output within the graph.
pub trait Sink {
    type Inputs: crate::input_tuple::InputTuple;
    type Key: Hash + Eq + Debug + Send + 'static;
    fn key(&self) -> Self::Key;
    fn emit(&self, inputs: <Self::Inputs as crate::input_tuple::InputTuple>::Ref<'_>);
}

/// An async compute node — spawns a future that produces a value.
pub trait AsyncCompute: Node {
    type Inputs: crate::input_tuple::InputTuple;
    fn compute(
        &self,
        inputs: <Self::Inputs as crate::input_tuple::InputTuple>::Ref<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'static>>;
}
