use std::{
    borrow::Cow,
    cmp::Ordering,
    fmt::{self, Debug},
    hash::{Hash, Hasher},
    sync::Arc,
};

pub enum ArcCow<'a, T: ?Sized> {
    Borrowed(&'a T),
    Owned(Arc<T>),
}

impl<T: ?Sized + PartialEq> PartialEq for ArcCow<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        let a = self.as_ref();
        let b = other.as_ref();
        a == b
    }
}

impl<T: ?Sized + PartialOrd> PartialOrd for ArcCow<'_, T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_ref().partial_cmp(other.as_ref())
    }
}

impl<T: ?Sized + Ord> Ord for ArcCow<'_, T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl<T: ?Sized + Eq> Eq for ArcCow<'_, T> {}

impl<T: ?Sized + Hash> Hash for ArcCow<'_, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Borrowed(borrowed) => Hash::hash(borrowed, state),
            Self::Owned(owned) => Hash::hash(&**owned, state),
        }
    }
}

impl<T: ?Sized> Clone for ArcCow<'_, T> {
    fn clone(&self) -> Self {
        match self {
            Self::Borrowed(borrowed) => Self::Borrowed(borrowed),
            Self::Owned(owned) => Self::Owned(owned.clone()),
        }
    }
}

impl<'a, T: ?Sized> From<&'a T> for ArcCow<'a, T> {
    fn from(value: &'a T) -> Self {
        Self::Borrowed(value)
    }
}

impl<T: ?Sized> From<Arc<T>> for ArcCow<'_, T> {
    fn from(value: Arc<T>) -> Self {
        Self::Owned(value)
    }
}

impl<T: ?Sized> From<&'_ Arc<T>> for ArcCow<'_, T> {
    fn from(value: &'_ Arc<T>) -> Self {
        Self::Owned(value.clone())
    }
}

impl From<String> for ArcCow<'_, str> {
    fn from(value: String) -> Self {
        Self::Owned(value.into())
    }
}

impl From<&String> for ArcCow<'_, str> {
    fn from(value: &String) -> Self {
        Self::Owned(value.clone().into())
    }
}

impl<'a> From<Cow<'a, str>> for ArcCow<'a, str> {
    fn from(value: Cow<'a, str>) -> Self {
        match value {
            Cow::Borrowed(borrowed) => Self::Borrowed(borrowed),
            Cow::Owned(owned) => Self::Owned(owned.into()),
        }
    }
}

impl<T> From<Vec<T>> for ArcCow<'_, [T]> {
    fn from(value: Vec<T>) -> Self {
        Self::Owned(Arc::from(value))
    }
}

impl<'a> From<&'a str> for ArcCow<'a, [u8]> {
    fn from(value: &'a str) -> Self {
        Self::Borrowed(value.as_bytes())
    }
}

impl<T: ?Sized + ToOwned> std::borrow::Borrow<T> for ArcCow<'_, T> {
    fn borrow(&self) -> &T {
        match self {
            Self::Borrowed(borrowed) => borrowed,
            Self::Owned(owned) => owned.as_ref(),
        }
    }
}

impl<T: ?Sized> std::ops::Deref for ArcCow<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => value.as_ref(),
        }
    }
}

impl<T: ?Sized> AsRef<T> for ArcCow<'_, T> {
    fn as_ref(&self) -> &T {
        match self {
            Self::Borrowed(borrowed) => borrowed,
            Self::Owned(owned) => owned.as_ref(),
        }
    }
}

impl<T: ?Sized + Debug> Debug for ArcCow<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Borrowed(borrowed) => Debug::fmt(borrowed, formatter),
            Self::Owned(owned) => Debug::fmt(&**owned, formatter),
        }
    }
}
