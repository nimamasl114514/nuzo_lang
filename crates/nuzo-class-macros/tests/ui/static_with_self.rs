use nuzo_class_macros::static_method;

struct Foo;

impl Foo {
    #[static_method]
    //~^ ERROR: #[static_method] must not take `self`
    fn create(&self) -> Foo {
        Foo
    }
}

fn main() {}
