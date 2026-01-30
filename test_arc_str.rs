use std::sync::Arc;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug)]
struct Test {
    name: Arc<str>,
}

fn main() {
    let t = Test { name: Arc::from("hello") };
    let j = serde_json::to_string(&t).unwrap();
    println!("{}", j);
    let t2: Test = serde_json::from_str(&j).unwrap();
    println!("{:?}", t2);
}
