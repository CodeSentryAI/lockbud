use std::sync::RwLock;

fn main() {
    let lock = RwLock::new(0);
    let _read = lock.read().unwrap();
    let _write = lock.write().unwrap(); // deadlocks with any concurrent reader
}
