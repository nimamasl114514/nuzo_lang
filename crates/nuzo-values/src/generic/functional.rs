//! Functor / Applicative / Monad — 函数式高阶 trait
//!
//! Rust 无 HKT，用 GAT associated type `Mapped<B>` 表达 F<A> -> F<B>。
//! 仅为核心 3 类型（Option/Vec/Result）实现，避免 HKT 复杂度。

/// Functor：可映射容器
pub trait Functor {
    type Item;
    type Mapped<B>;
    fn map<B, F: FnMut(Self::Item) -> B>(self, f: F) -> Self::Mapped<B>;
}

/// Applicative：可应用包装的函数
pub trait Applicative: Functor {
    fn pure(value: Self::Item) -> Self;
    fn apply<B, F: FnMut(Self::Item) -> B>(self, f: Self::Mapped<F>) -> Self::Mapped<B>;
}

/// Monad：可绑定链式操作
pub trait Monad: Applicative {
    fn bind<B, F: FnMut(Self::Item) -> Self::Mapped<B>>(self, f: F) -> Self::Mapped<B>;
}

// ===== impl for Option<T> =====

impl<T> Functor for Option<T> {
    type Item = T;
    type Mapped<B> = Option<B>;
    fn map<B, F: FnMut(Self::Item) -> B>(self, f: F) -> Option<B> {
        self.map(f)
    }
}

impl<T> Applicative for Option<T> {
    fn pure(value: Self::Item) -> Self {
        Some(value)
    }
    fn apply<B, F: FnMut(Self::Item) -> B>(self, f: Self::Mapped<F>) -> Option<B> {
        match (self, f) {
            (Some(x), Some(mut g)) => Some(g(x)),
            _ => None,
        }
    }
}

impl<T> Monad for Option<T> {
    fn bind<B, F: FnMut(Self::Item) -> Self::Mapped<B>>(self, f: F) -> Option<B> {
        self.and_then(f)
    }
}

// ===== impl for Vec<T> =====

impl<T> Functor for Vec<T> {
    type Item = T;
    type Mapped<B> = Vec<B>;
    fn map<B, F: FnMut(Self::Item) -> B>(self, f: F) -> Vec<B> {
        self.into_iter().map(f).collect()
    }
}

impl<T: Clone> Applicative for Vec<T> {
    fn pure(value: Self::Item) -> Self {
        vec![value]
    }
    fn apply<B, F: FnMut(Self::Item) -> B>(self, fs: Self::Mapped<F>) -> Vec<B> {
        let mut fs = fs;
        self.into_iter()
            .flat_map(|x| fs.iter_mut().map(|g| g(x.clone())).collect::<Vec<_>>())
            .collect()
    }
}

impl<T: Clone> Monad for Vec<T> {
    fn bind<B, F: FnMut(Self::Item) -> Self::Mapped<B>>(self, f: F) -> Vec<B> {
        self.into_iter().flat_map(f).collect()
    }
}

// ===== impl for Result<T, E> =====

impl<T, E> Functor for Result<T, E> {
    type Item = T;
    type Mapped<B> = Result<B, E>;
    fn map<B, F: FnMut(Self::Item) -> B>(self, f: F) -> Result<B, E> {
        self.map(f)
    }
}

impl<T, E> Applicative for Result<T, E> {
    fn pure(value: Self::Item) -> Self {
        Ok(value)
    }
    fn apply<B, F: FnMut(Self::Item) -> B>(self, f: Self::Mapped<F>) -> Result<B, E> {
        match (self, f) {
            (Ok(x), Ok(mut g)) => Ok(g(x)),
            (Err(e), _) => Err(e),
            (_, Err(e)) => Err(e),
        }
    }
}

impl<T, E> Monad for Result<T, E> {
    fn bind<B, F: FnMut(Self::Item) -> Self::Mapped<B>>(self, f: F) -> Result<B, E> {
        self.and_then(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_option_functor() {
        let x: Option<i32> = Some(1);
        assert_eq!(x.map(|v| v + 1), Some(2));
        let x: Option<i32> = None;
        assert_eq!(x.map(|v| v + 1), None);
    }

    #[test]
    fn test_vec_monad() {
        let v = vec![1, 2, 3];
        let r: Vec<i32> = v.bind(|x| vec![x, x * 10]);
        assert_eq!(r, vec![1, 10, 2, 20, 3, 30]);
    }

    #[test]
    fn test_result_monad() {
        let r: Result<i32, &str> = Ok(1);
        let r: Result<i32, &str> = r.bind(|x| if x > 0 { Ok(x + 1) } else { Err("neg") });
        assert_eq!(r, Ok(2));
    }
}
