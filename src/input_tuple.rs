/// Maps a tuple of value types to a tuple of references.
/// Used by Compute, AsyncCompute, and Sink to express their input signature.
pub trait InputTuple {
    type Ref<'a>;
}

/// Generates InputTuple impls for tuples of increasing arity.
macro_rules! impl_input_tuple {
    // Base case: single element tuple
    (($($T:ident),+)) => {
        impl<$($T: Send + 'static),+> InputTuple for ($($T,)+) {
            type Ref<'a> = ($(&'a $T,)+);
        }
    };
}

impl_input_tuple!((A));
impl_input_tuple!((A, B));
impl_input_tuple!((A, B, Cc));
impl_input_tuple!((A, B, Cc, D));
impl_input_tuple!((A, B, Cc, D, E));
impl_input_tuple!((A, B, Cc, D, E, Ff));
impl_input_tuple!((A, B, Cc, D, E, Ff, G));
impl_input_tuple!((A, B, Cc, D, E, Ff, G, H));
impl_input_tuple!((A, B, Cc, D, E, Ff, G, H, I));
impl_input_tuple!((A, B, Cc, D, E, Ff, G, H, I, J));
impl_input_tuple!((A, B, Cc, D, E, Ff, G, H, I, J, K));
impl_input_tuple!((A, B, Cc, D, E, Ff, G, H, I, J, K, L));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_tuple_arities() {
        // Verify that the trait is implemented for arities 1-4
        fn assert_input_tuple<T: InputTuple>() {}

        assert_input_tuple::<(f64,)>();
        assert_input_tuple::<(f64, f64)>();
        assert_input_tuple::<(f64, String, i32)>();
        assert_input_tuple::<(f64, String, i32, bool)>();
    }
}
