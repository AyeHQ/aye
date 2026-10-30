//! A count of things in words ([`plural`]).

pub(crate) fn plural<N: Copy + std::fmt::Display + PartialEq + From<u8>>(
    n: N,
    one: &str,
    many: &str,
) -> String {
    let word = if n == N::from(1) { one } else { many };
    format!("{n} {word}")
}
