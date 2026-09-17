//! A tiny `bitflags`-style macro so the crate stays dependency-free. Every
//! generated flag type is a `Copy` newtype over an unsigned integer with
//! `const` set operations usable in `static` initializers.

macro_rules! bitflags {
    (
        $(#[$outer:meta])*
        pub struct $name:ident: $repr:ty {
            $(
                $(#[$inner:meta])*
                const $flag:ident = $value:expr;
            )*
        }
    ) => {
        $(#[$outer])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $name($repr);

        impl $name {
            /// No bits set.
            pub const EMPTY: Self = Self(0);
            $(
                $(#[$inner])*
                pub const $flag: Self = Self($value);
            )*

            /// The raw bit pattern.
            pub const fn bits(self) -> $repr {
                self.0
            }

            /// Wrap a raw bit pattern (unknown bits are kept).
            pub const fn from_bits_retain(bits: $repr) -> Self {
                Self(bits)
            }

            /// Whether no bits are set.
            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            /// Whether every bit of `other` is set in `self`.
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            /// Whether any bit of `other` is set in `self`.
            pub const fn intersects(self, other: Self) -> bool {
                self.0 & other.0 != 0
            }

            /// Set union (`const`, so usable in `static` initializers).
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            /// Set intersection.
            pub const fn intersection(self, other: Self) -> Self {
                Self(self.0 & other.0)
            }

            /// Set difference: the bits of `self` not in `other`.
            pub const fn difference(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }

            /// Every `(name, flag)` pair, in declaration order.
            pub const FLAGS: &'static [(&'static str, Self)] = &[
                $((stringify!($flag), Self::$flag),)*
            ];
        }

        impl core::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self {
                self.union(rhs)
            }
        }

        impl core::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }

        impl core::ops::BitAnd for $name {
            type Output = Self;
            fn bitand(self, rhs: Self) -> Self {
                self.intersection(rhs)
            }
        }

        impl core::ops::Sub for $name {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self {
                self.difference(rhs)
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}(", stringify!($name))?;
                let mut rest = self.0;
                let mut first = true;
                for (name, flag) in Self::FLAGS {
                    // Skip composite aliases so `bool` prints as FALSE | TRUE.
                    if flag.0.count_ones() != 1 {
                        continue;
                    }
                    if rest & flag.0 != 0 {
                        if !first {
                            f.write_str(" | ")?;
                        }
                        f.write_str(name)?;
                        first = false;
                        rest &= !flag.0;
                    }
                }
                if rest != 0 {
                    if !first {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{rest:#x}")?;
                    first = false;
                }
                if first {
                    f.write_str("EMPTY")?;
                }
                f.write_str(")")
            }
        }
    };
}

pub(crate) use bitflags;
