use nuzo_class_macros::{class_impl, get};

struct AllSkipped {
    x: i32,
}

#[class_impl(serialize)]
impl AllSkipped {
    #[get(skip)]
    fn x(&self) -> i32 {
        self.x
    }
}

fn main() {}
