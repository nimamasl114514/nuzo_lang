// Temporary bytecode dump tool
use nuzo_compiler::Compiler;

fn main() {
    let source = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let chunk = Compiler::compile(&source).unwrap();
    println!("{}", chunk.disassemble());
}
