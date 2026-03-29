#[derive(Clone, Debug, Default)]
pub struct Conda;

impl<E> From<&E> for Conda {
    fn from(_value: &E) -> Self {
        Self
    }
}
