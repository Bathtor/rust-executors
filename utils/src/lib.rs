// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

/// A simple method to explicitly throw away return parameters.
///
/// # Examples
///
/// Ignoring a Result.
///
/// ```
/// use utils::*;
/// let res: Result<(), String> = Ok(());
/// ignore(res);
/// ```
#[inline(always)]
pub fn ignore<V>(_: V) -> () {
    ()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn ignore_primitives() {
        assert_eq!(ignore(2 + 2), ());
    }
    
    struct SomeStruct {
        a: u32,
        b: f64,
        c: bool,
    }
    
    #[test]
    fn ignore_objects() {
        let v = SomeStruct { a: 1, b: 2.0, c: true};
        assert_eq!(ignore(v), ());
    }
}
