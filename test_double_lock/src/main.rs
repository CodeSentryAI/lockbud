use std::sync::Mutex;

fn main() {
    let m = Mutex::new(0);
    let _g1 = m.lock().unwrap();
    let _g2 = m.lock().unwrap(); // double lock here
    println!("{}", *_g2);
}
