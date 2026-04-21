use std::sync::mpsc::channel;

fn main() {
    let (_tx, rx) = channel::<i32>();
    rx.recv().unwrap(); // orphan receiver, sender never sends
}
