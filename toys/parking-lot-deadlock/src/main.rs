use parking_lot::Mutex;

fn main() {
    let m = Mutex::new(0);
    let _g1 = m.lock();
    let _g2 = m.lock(); // deadlock
}
