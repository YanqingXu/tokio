//! 在Tokio crate中本地记录的类型，但实际上并不存在于此处。
//!
//! **注意** 这个模块只在docs.rs上可见，你不能直接在你自己的代码中使用它。

/// 这里没有定义的类型的名称。
///
/// 这通常用作另一种类型的别名，如下所示：
///
/// ```rust,ignore
/// /// 参见[some::other::location](https://example.com)。
/// type DEFINED_ELSEWHERE = crate::doc::NotDefinedHere;
/// ```
///
/// 这种类型是不可实例化的，就像 [`never` 类型]一样，以确保没有人会意外地使用它。
///
/// [`never` 类型]: https://doc.rust-lang.org/std/primitive.never.html
#[derive(Debug)]
pub enum NotDefinedHere {}

#[cfg(feature = "net")]
impl mio::event::Source for NotDefinedHere {
    fn register(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        Ok(())
    }
    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        Ok(())
    }
    fn deregister(&mut self, registry: &mio::Registry) -> std::io::Result<()> {
        Ok(())
    }
}

/// 这部分代码使用了条件编译（#[cfg(feature = "net")]），这意味着只有在启用了 net 特性时，才会编译这部分代码。
/// 在这里，为 NotDefinedHere 类型实现了 mio::event::Source 特性，但由于 NotDefinedHere 是不可实例化的，这实际上不会在运行时有任何作用。

#[cfg(feature = "net")]
pub mod os;
