//! A runnable example demonstrating all `nuzo_class` features.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p nuzo_class --example person
//! ```

// The method macros are consumed inside `#[class_impl]` and therefore not
// directly visible to the unused-imports lint.
#[allow(unused_imports)]
use nuzo_class::{class, class_impl, constructor, get, method, set, static_method};

#[class(debug, default, clone)]
struct Person {
    name: String,
    age: u32,
}

#[class_impl]
impl Person {
    #[constructor]
    fn new(name: String, age: u32) -> Self {
        Self { name, age }
    }

    #[get]
    fn name(&self) -> &str {
        &self.name
    }

    #[get]
    fn age(&self) -> u32 {
        self.age
    }

    #[set]
    fn set_age(&mut self, age: u32) {
        self.age = age;
    }

    #[method]
    fn greet(&self) -> String {
        format!("Hello, my name is {} and I am {} years old.", self.name, self.age)
    }

    #[static_method]
    fn species() -> &'static str {
        "Homo sapiens"
    }
}

fn main() {
    let mut person = Person::new("Alice".to_string(), 30);

    assert_eq!(person.name(), "Alice");
    assert_eq!(person.age(), 30);

    person.set_age(31);
    assert_eq!(person.age(), 31);

    let greeting = person.greet();
    assert!(greeting.contains("Alice"));
    assert!(greeting.contains("31"));

    assert_eq!(Person::species(), "Homo sapiens");

    println!("Person example ran successfully: {}", greeting);
}
