use nuzo_class_macros::get;

struct Foo;

impl Foo {
    #[get]
    //~^ ERROR: #[get] method must take `&self` as the first argument
    fn value() -> i32 {
        0
    }
}

fn main() {}
