pub trait Memento<T> {
    fn restore(self) -> T;
}

pub trait Originator<T> {
    fn snapshot(&self) -> T;
}
