pub(crate) trait Memento<T> {
    fn restore(self) -> T;
}

pub(crate) trait Originator<T> {
    fn snapshot(&self) -> T;
}
