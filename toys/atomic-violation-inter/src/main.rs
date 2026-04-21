use std::sync::atomic::{AtomicI32, Ordering};

fn load_check(a: &AtomicI32) -> bool {
    a.load(Ordering::Relaxed) == 0
}

fn store_value(a: &AtomicI32) {
    a.store(1, Ordering::Relaxed);
}

fn main() {
    let a = AtomicI32::new(0);
    if load_check(&a) {
        store_value(&a);
    }
}
