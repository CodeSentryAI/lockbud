use std::sync::mpsc::sync_channel;

fn main() {
    let (tx, rx) = sync_channel(0);
    tx.send(42).unwrap(); // blocks forever: no receiver has been reached yet
    rx.recv().unwrap();
}
