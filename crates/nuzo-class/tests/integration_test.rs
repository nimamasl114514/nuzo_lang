//! Integration tests for `nuzo_class`.
//!
//! These tests exercise the class macros (`#[class]`, `#[class_impl]`,
//! `#[constructor]`, `#[get]`, `#[set]`, `#[method]`, `#[static_method]`)
//! across typical usage scenarios.

#![allow(dead_code)]

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

#[test]
fn constructor_works() {
    let person = Person::new("Alice".to_string(), 30);
    assert_eq!(person.name(), "Alice");
    assert_eq!(person.age(), 30);
}

#[test]
fn getter_works() {
    let person = Person::new("Bob".to_string(), 25);
    assert_eq!(person.name(), "Bob");
    assert_eq!(person.age(), 25);
}

#[test]
fn setter_works() {
    let mut person = Person::new("Carol".to_string(), 40);
    person.set_age(41);
    assert_eq!(person.age(), 41);
}

#[test]
fn instance_method_works() {
    let person = Person::new("Dave".to_string(), 22);
    let greeting = person.greet();
    assert!(greeting.contains("Dave"));
    assert!(greeting.contains("22"));
}

#[test]
fn static_method_works() {
    assert_eq!(Person::species(), "Homo sapiens");
}

#[class]
struct EmptyImpl;

#[class_impl]
impl EmptyImpl {}

#[test]
fn empty_impl_block_works() {
    let _ = EmptyImpl;
}

#[class]
struct Point {
    x: f64,
    y: f64,
    z: f64,
}

#[class_impl]
impl Point {
    #[constructor]
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[get]
    fn x(&self) -> f64 {
        self.x
    }

    #[get]
    fn y(&self) -> f64 {
        self.y
    }

    #[get]
    fn z(&self) -> f64 {
        self.z
    }

    #[method]
    fn distance_from_origin(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

#[test]
fn multiple_fields_class_works() {
    let point = Point::new(3.0, 4.0, 0.0);
    assert_eq!(point.x(), 3.0);
    assert_eq!(point.y(), 4.0);
    assert_eq!(point.z(), 0.0);
    assert_eq!(point.distance_from_origin(), 5.0);
}

#[class(debug, default, clone)]
struct Wrapper<T> {
    value: T,
}

#[class_impl]
impl<T> Wrapper<T> {
    #[constructor]
    fn new(value: T) -> Self {
        Self { value }
    }

    #[get]
    fn value(&self) -> &T {
        &self.value
    }

    #[set]
    fn set_value(&mut self, value: T) {
        self.value = value;
    }

    #[method]
    fn describe(&self) -> String
    where
        T: std::fmt::Debug,
    {
        format!("Wrapper holds: {:?}", self.value)
    }

    #[static_method]
    fn label() -> &'static str {
        "Wrapper"
    }
}

#[test]
fn generic_class_works() {
    let mut wrapper = Wrapper::new(42);
    assert_eq!(*wrapper.value(), 42);
    assert_eq!(wrapper.describe(), "Wrapper holds: 42");
    assert_eq!(Wrapper::<i32>::label(), "Wrapper");

    wrapper.set_value(100);
    assert_eq!(*wrapper.value(), 100);

    let string_wrapper = Wrapper::new("hello".to_string());
    assert_eq!(string_wrapper.value(), "hello");
}

#[test]
fn class_derives_work() {
    let person = Person::new("Eve".to_string(), 28);

    // Debug
    let debug_repr = format!("{:?}", person);
    assert!(debug_repr.contains("Person"));
    assert!(debug_repr.contains("Eve"));

    // Default
    let default_person = Person::default();
    assert_eq!(default_person.name(), "");
    assert_eq!(default_person.age(), 0);

    // Clone
    let cloned = person.clone();
    assert_eq!(cloned.name(), "Eve");
    assert_eq!(cloned.age(), 28);
}

#[class(debug, clone)]
struct SerdePerson {
    name: String,
    age: u32,
}

#[class_impl(serialize, deserialize)]
impl SerdePerson {
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
        format!("Hi, I'm {} (age {})", self.name, self.age)
    }
}

#[test]
fn serialize_works() {
    let person = SerdePerson::new("Alice".to_string(), 30);
    let json = serde_json::to_string(&person).unwrap();
    assert!(json.contains("\"name\":\"Alice\""));
    assert!(json.contains("\"age\":30"));
}

#[test]
fn deserialize_works() {
    let json = r#"{"name":"Bob","age":25}"#;
    let person: SerdePerson = serde_json::from_str(json).unwrap();
    assert_eq!(person.name(), "Bob");
    assert_eq!(person.age(), 25);
}

#[test]
fn deserialize_with_setter_override_works() {
    let json = r#"{"name":"Carol","age":40}"#;
    let mut person: SerdePerson = serde_json::from_str(json).unwrap();
    assert_eq!(person.age(), 40);
    person.set_age(41);
    assert_eq!(person.age(), 41);
}

#[test]
fn roundtrip_serde_works() {
    let original = SerdePerson::new("Dave".to_string(), 28);
    let json = serde_json::to_string(&original).unwrap();
    let restored: SerdePerson = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name(), "Dave");
    assert_eq!(restored.age(), 28);
}

#[class(debug, default, clone)]
struct Config {
    host: String,
    port: u16,
}

#[class_impl(serialize, deserialize)]
impl Config {
    #[get]
    fn host(&self) -> &str {
        &self.host
    }

    #[get]
    fn port(&self) -> u16 {
        self.port
    }

    #[set]
    fn set_host(&mut self, host: String) {
        self.host = host;
    }

    #[set]
    fn set_port(&mut self, port: u16) {
        self.port = port;
    }
}

#[test]
fn deserialize_with_default_and_setters() {
    let json = r#"{"host":"localhost","port":8080}"#;
    let config: Config = serde_json::from_str(json).unwrap();
    assert_eq!(config.host(), "localhost");
    assert_eq!(config.port(), 8080);
}

#[test]
fn serialize_with_getters_only() {
    let mut config = Config::default();
    config.set_host("127.0.0.1".to_string());
    config.set_port(3000);
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("\"host\":\"127.0.0.1\""));
    assert!(json.contains("\"port\":3000"));
}

#[class(debug, clone)]
struct RenamedPerson {
    first_name: String,
    last_name: String,
}

#[class_impl(serialize, deserialize)]
impl RenamedPerson {
    #[constructor]
    fn new(first_name: String, last_name: String) -> Self {
        Self { first_name, last_name }
    }

    #[get(rename = "firstName")]
    fn first_name(&self) -> &str {
        &self.first_name
    }

    #[get(rename = "lastName")]
    fn last_name(&self) -> &str {
        &self.last_name
    }

    #[set(rename_from = "firstName")]
    fn set_first_name(&mut self, first_name: String) {
        self.first_name = first_name;
    }

    #[set(rename_from = "lastName")]
    fn set_last_name(&mut self, last_name: String) {
        self.last_name = last_name;
    }
}

#[test]
fn rename_serialize_uses_custom_names() {
    let person = RenamedPerson::new("Alice".to_string(), "Smith".to_string());
    let json = serde_json::to_string(&person).unwrap();
    assert!(json.contains("\"firstName\":\"Alice\""));
    assert!(json.contains("\"lastName\":\"Smith\""));
    // Should NOT contain snake_case
    assert!(!json.contains("\"first_name\""));
}

#[test]
fn rename_deserialize_uses_custom_names() {
    let json = r#"{"firstName":"Bob","lastName":"Jones"}"#;
    let person: RenamedPerson = serde_json::from_str(json).unwrap();
    assert_eq!(person.first_name(), "Bob");
    assert_eq!(person.last_name(), "Jones");
}

#[test]
fn rename_roundtrip() {
    let original = RenamedPerson::new("Carol".to_string(), "White".to_string());
    let json = serde_json::to_string(&original).unwrap();
    let restored: RenamedPerson = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.first_name(), "Carol");
    assert_eq!(restored.last_name(), "White");
}

#[class(debug, default, clone)]
struct ComputedFields {
    base: u32,
    multiplier: u32,
}

#[class_impl(serialize, deserialize)]
impl ComputedFields {
    #[constructor]
    fn new(base: u32, multiplier: u32) -> Self {
        Self { base, multiplier }
    }

    #[get]
    fn base(&self) -> u32 {
        self.base
    }

    #[get]
    fn multiplier(&self) -> u32 {
        self.multiplier
    }

    #[get(skip)]
    fn computed(&self) -> u32 {
        self.base * self.multiplier
    }

    #[set]
    fn set_base(&mut self, base: u32) {
        self.base = base;
    }

    #[set]
    fn set_multiplier(&mut self, multiplier: u32) {
        self.multiplier = multiplier;
    }
}

#[test]
fn skip_field_not_in_serialized_output() {
    let obj = ComputedFields::new(3, 4);
    let json = serde_json::to_string(&obj).unwrap();
    assert!(json.contains("\"base\":3"));
    assert!(json.contains("\"multiplier\":4"));
    // "computed" should NOT appear
    assert!(!json.contains("computed"));
}

#[test]
fn skip_field_still_callable() {
    let obj = ComputedFields::new(3, 4);
    assert_eq!(obj.computed(), 12);
}

#[class(debug, clone)]
struct GenericWrapper<T> {
    value: T,
}

#[class_impl(serialize, deserialize)]
impl<T> GenericWrapper<T> {
    #[constructor]
    fn new(value: T) -> Self {
        Self { value }
    }

    #[get]
    fn value(&self) -> &T {
        &self.value
    }

    #[set]
    fn set_value(&mut self, value: T) {
        self.value = value;
    }
}

#[test]
fn generic_serde_auto_bounds() {
    let original = GenericWrapper::new(42i32);
    let json = serde_json::to_string(&original).unwrap();
    let restored: GenericWrapper<i32> = serde_json::from_str(&json).unwrap();
    assert_eq!(*restored.value(), 42);
}

#[test]
fn generic_serde_with_string() {
    let original = GenericWrapper::new("hello".to_string());
    let json = serde_json::to_string(&original).unwrap();
    let restored: GenericWrapper<String> = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.value(), "hello");
}
