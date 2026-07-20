use nuzo_class_macros::set;

struct Foo;

impl Foo {
    #[set]
    //~^ ERROR: #[set] method first argument must be `&mut self`
    fn value(&self, v: i32) {
    }
}

fn main() {}
