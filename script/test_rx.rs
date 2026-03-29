fn main() {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<i32>();
    let result = rx.try_next();
    match result {
        Ok(Some(v)) => println!("value: {}", v),
        Ok(None) => println!("closed"),
        Err(e) => println!("err: {:?}", e),
    }
}
