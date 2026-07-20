use nuzo_class_macros::class_impl;

struct Foo;

#[class_impl]
//~^ ERROR: multiple class attributes on the same method
//~| ERROR: previous class attribute here
impl Foo {
    #[get]
    #[set]
    fn value(&self, v: i32) -> i32 {
        v
    }
}

fn main() {}
