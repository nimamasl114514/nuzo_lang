//! 银行账户示例 — 展示 nuzo_class 在业务场景中的用法。
//!
//! ```bash
//! cargo run -p nuzo_class --example bank_account
//! ```

#[allow(unused_imports)]
use nuzo_class::{class, class_impl, constructor, get, method, set, static_method};

#[class(debug, clone)]
struct BankAccount {
    owner: String,
    balance: f64,
}

#[class_impl]
impl BankAccount {
    #[constructor]
    fn new(owner: String, initial_deposit: f64) -> Self {
        assert!(initial_deposit >= 0.0, "初始存款不能为负数");
        Self { owner, balance: initial_deposit }
    }

    #[get]
    fn owner(&self) -> &str {
        &self.owner
    }

    #[get]
    fn balance(&self) -> f64 {
        self.balance
    }

    #[method]
    fn deposit(&mut self, amount: f64) -> String {
        assert!(amount > 0.0, "存款金额必须大于零");
        self.balance += amount;
        format!("存入 ¥{:.2}，余额 ¥{:.2}", amount, self.balance)
    }

    #[method]
    fn withdraw(&mut self, amount: f64) -> Result<String, &'static str> {
        if amount <= 0.0 {
            return Err("取款金额必须大于零");
        }
        if amount > self.balance {
            return Err("余额不足");
        }
        self.balance -= amount;
        Ok(format!("取出 ¥{:.2}，余额 ¥{:.2}", amount, self.balance))
    }

    #[method]
    fn transfer(&mut self, target: &mut BankAccount, amount: f64) -> Result<String, &'static str> {
        if amount > self.balance {
            return Err("余额不足，无法转账");
        }
        self.balance -= amount;
        target.balance += amount;
        Ok(format!(
            "从 {} 转账 ¥{:.2} 到 {}，己方余额 ¥{:.2}",
            self.owner, amount, target.owner, self.balance
        ))
    }

    #[static_method]
    fn bank_name() -> &'static str {
        "Nuzo Bank"
    }
}

fn main() {
    println!("=== {} ===\n", BankAccount::bank_name());

    let mut alice = BankAccount::new("Alice".to_string(), 1000.0);
    let mut bob = BankAccount::new("Bob".to_string(), 500.0);

    println!("{} 开户，余额 ¥{:.2}", alice.owner(), alice.balance());
    println!("{} 开户，余额 ¥{:.2}", bob.owner(), bob.balance());

    println!("\n--- 存款 ---");
    println!("{}", alice.deposit(200.0));

    println!("\n--- 取款 ---");
    match alice.withdraw(150.0) {
        Ok(msg) => println!("{}", msg),
        Err(e) => println!("取款失败: {}", e),
    }

    println!("\n--- 转账 ---");
    match alice.transfer(&mut bob, 300.0) {
        Ok(msg) => println!("{}", msg),
        Err(e) => println!("转账失败: {}", e),
    }

    println!("\n--- 最终余额 ---");
    println!("{}: ¥{:.2}", alice.owner(), alice.balance());
    println!("{}: ¥{:.2}", bob.owner(), bob.balance());

    println!("\n--- Debug 输出（#[class(debug)] 生成） ---");
    println!("{:?}", alice);
    println!("{:?}", bob);
}
