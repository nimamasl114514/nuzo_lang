use nuzo_class_macros::constructor;

struct Foo;

impl Foo {
    #[constructor]
    //~^ ERROR: #[constructor] must return `Self`
    fn new() -> u32 {
        Foo
    }
}

fn main() {}
