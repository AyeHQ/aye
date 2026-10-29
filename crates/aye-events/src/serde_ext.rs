//! How the wire spells what serde has no attribute for: durations as
//! fractional milliseconds, the zeros left out, and the enums written as
//! one string per variant that keep any other string.

/// Durations are fractional milliseconds on the wire, under `*_ms` keys.
pub(crate) mod millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64((d.as_secs_f64() * 1_000_000.0).round() / 1000.0)
    }

    /// Negative values are zero and values too big for a `Duration` (a
    /// test that patched the clock) saturate: a worker's numbers must never
    /// panic the runner.
    /// A missing or `null` duration (a worker whose clock went non-finite)
    /// reads as zero rather than losing the record.
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let Some(ms) = Option::<f64>::deserialize(d)? else {
            return Ok(Duration::ZERO);
        };
        Ok(Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).unwrap_or(Duration::MAX))
    }

    /// The same, for a duration that may be absent.
    pub(crate) mod optional {
        use std::time::Duration;

        use serde::{Deserializer, Serializer};

        #[expect(clippy::ref_option, reason = "signature fixed by serde")]
        pub(crate) fn serialize<S: Serializer>(
            d: &Option<Duration>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match d {
                Some(d) => super::serialize(d, s),
                None => s.serialize_none(),
            }
        }

        /// A `null` reads as absent; see [`super::deserialize`] otherwise.
        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Duration>, D::Error> {
            let ms = <Option<f64> as serde::Deserialize>::deserialize(d)?;
            Ok(ms.map(|ms| {
                Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).unwrap_or(Duration::MAX)
            }))
        }
    }

    #[cfg(test)]
    mod tests {
        use std::time::Duration;

        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Timed {
            #[serde(with = "super")]
            ms: Duration,
        }

        fn parse(ms: &str) -> Duration {
            serde_json::from_str::<Timed>(&format!(r#"{{"ms":{ms}}}"#))
                .unwrap()
                .ms
        }

        #[test]
        fn durations_out_of_range_saturate_instead_of_panicking() {
            assert_eq!(parse("1.5"), Duration::from_micros(1500));
            assert_eq!(parse("-3"), Duration::ZERO);
            assert_eq!(parse("3e22"), Duration::MAX);
            assert_eq!(parse("1e303"), Duration::MAX);
        }

        #[test]
        fn a_saturated_duration_serializes_as_a_finite_number() {
            let mut out = Vec::new();
            super::serialize(&Duration::MAX, &mut serde_json::Serializer::new(&mut out)).unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(text.parse::<f64>().unwrap().is_finite(), "{text}");
        }
    }
}

/// For `skip_serializing_if`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
pub(crate) const fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
pub(crate) const fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

/// Declares an enum written on the wire as one string per variant, and
/// adds a last variant, `Other(Box<str>)`, that keeps any other string: a
/// value a worker or a stored log holds that this aye doesn't know still
/// reads, and is written back as it was. The enum gets `ALL`, its known
/// values in order (all but `Other`), `as_str`, `Display`, a `From<&str>`
/// and a `FromStr` that never fail, and serde as the string.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $text:literal,
            )+
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
            /// A value this aye doesn't know, as it was written.
            Other(Box<str>),
        }

        impl $name {
            /// Every known value, in order: all but `Other`.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// How it is written.
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Other(text) => text,
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        /// The known value `text` spells, else `Other`.
        impl From<&str> for $name {
            fn from(text: &str) -> Self {
                match text {
                    $($text => Self::$variant,)+
                    other => Self::Other(other.into()),
                }
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::std::convert::Infallible;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Ok(Self::from(text))
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Ok(Self::from(text.as_str()))
            }
        }
    };
}

pub(crate) use string_enum;
