#[derive(Clone, Debug, Default)]
pub struct Poetry;

impl<E> From<&E> for Poetry {
    fn from(_value: &E) -> Self {
        Self
    }
}
