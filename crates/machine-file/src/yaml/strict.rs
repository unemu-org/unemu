// SPDX-License-Identifier: AGPL-3.0-or-later
//! Recursive serde adapter preserving scalar types in YAML integer/bool fields.
//! See the machine-files design § YAML: comparison. String fields still use
//! YAML's typed string decoder, so an unquoted CPU model remains a string.

use super::{integer_spelling, position, Source};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::{cell::RefCell, fmt};

pub(super) struct Strict<'a, D> {
    pub inner: D,
    pub source: &'a Source,
    pub expected: &'a RefCell<Option<String>>,
}
struct Wrapped<'a, V> {
    inner: V,
    source: &'a Source,
    expected: &'a RefCell<Option<String>>,
}
struct Expecting<'a, V>(&'a V);
impl<'de, V: Visitor<'de>> fmt::Display for Expecting<'_, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.expecting(f)
    }
}

macro_rules! delegate {
    ($($method:ident $(($($arg:ident: $ty:ty),*))?),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, $($($arg: $ty,)*)? visitor: V) -> Result<V::Value, Self::Error> {
            {
                let expected = Expecting(&visitor).to_string();
                self.inner.$method($($($arg,)*)? Wrapped { inner: visitor, source: self.source, expected: self.expected }).map_err(|error| {
                    self.expected.borrow_mut().get_or_insert(expected);
                    error
                })
            }
        }
    )*};
}

macro_rules! typed_scalars {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            let scalar = serde_saphyr::Spanned::<Scalar>::deserialize(self.inner)?;
            if let Some(message) = position(scalar.defined).and_then(|p| self.source.scalars.get(&p)).and_then(|text| integer_spelling(text)) {
                return Err(de::Error::custom(message));
            }
            scalar.value.visit(visitor)
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Strict<'_, D> {
    type Error = D::Error;
    typed_scalars! { deserialize_i8, deserialize_i16, deserialize_i32, deserialize_i64,
    deserialize_i128, deserialize_u8, deserialize_u16, deserialize_u32,
    deserialize_u64, deserialize_u128 }
    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_any(visitor)
    }
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.inner.deserialize_any(Wrapped {
            inner: visitor,
            source: self.source,
            expected: self.expected,
        })
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        // These are saphyr's synthetic position records, whose deserializers
        // intentionally expose only the struct entry point.
        if matches!(name, "Location" | "Span") {
            self.inner.deserialize_struct(name, fields, visitor)
        } else {
            self.inner.deserialize_any(Wrapped {
                inner: visitor,
                source: self.source,
                expected: self.expected,
            })
        }
    }

    delegate! {
        deserialize_any, deserialize_f32, deserialize_f64, deserialize_char,
        deserialize_str, deserialize_string, deserialize_bytes, deserialize_byte_buf,
        deserialize_option, deserialize_unit,
        deserialize_unit_struct(name: &'static str),
        deserialize_newtype_struct(name: &'static str),
        deserialize_tuple(len: usize),
        deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map,
        deserialize_enum(name: &'static str, variants: &'static [&'static str]),
        deserialize_identifier, deserialize_ignored_any
    }
}

macro_rules! scalar_visits {
    ($($method:ident($ty:ty)),* $(,)?) => {$(
        fn $method<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
            self.inner.$method(value)
        }
    )*};
}

impl<'de, V: Visitor<'de>> Visitor<'de> for Wrapped<'_, V> {
    type Value = V::Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(f)
    }
    scalar_visits! {
        visit_bool(bool), visit_i8(i8), visit_i16(i16), visit_i32(i32), visit_i64(i64),
        visit_i128(i128), visit_u8(u8), visit_u16(u16), visit_u32(u32), visit_u64(u64),
        visit_u128(u128), visit_f32(f32), visit_f64(f64), visit_char(char),
        visit_str(&str), visit_borrowed_str(&'de str), visit_string(String),
        visit_bytes(&[u8]), visit_borrowed_bytes(&'de [u8]), visit_byte_buf(Vec<u8>)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_none()
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_some(Strict {
            inner: d,
            source: self.source,
            expected: self.expected,
        })
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_newtype_struct(Strict {
            inner: d,
            source: self.source,
            expected: self.expected,
        })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_seq(Wrapped {
            inner: access,
            source: self.source,
            expected: self.expected,
        })
    }
    fn visit_map<A: MapAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_map(Wrapped {
            inner: access,
            source: self.source,
            expected: self.expected,
        })
    }
    fn visit_enum<A: de::EnumAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_enum(Wrapped {
            inner: access,
            source: self.source,
            expected: self.expected,
        })
    }
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Wrapped<'_, S> {
    type Value = S::Value;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.inner.deserialize(Strict {
            inner: d,
            source: self.source,
            expected: self.expected,
        })
    }
}
impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Wrapped<'_, A> {
    type Error = A::Error;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.inner.next_element_seed(Wrapped {
            inner: seed,
            source: self.source,
            expected: self.expected,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}
impl<'de, A: MapAccess<'de>> MapAccess<'de> for Wrapped<'_, A> {
    type Error = A::Error;
    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.inner.next_key_seed(Wrapped {
            inner: seed,
            source: self.source,
            expected: self.expected,
        })
    }
    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        self.inner.next_value_seed(Wrapped {
            inner: seed,
            source: self.source,
            expected: self.expected,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

impl<'a, 'de, A: de::EnumAccess<'de>> de::EnumAccess<'de> for Wrapped<'a, A> {
    type Error = A::Error;
    type Variant = Wrapped<'a, A::Variant>;
    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let (value, variant) = self.inner.variant_seed(Wrapped {
            inner: seed,
            source: self.source,
            expected: self.expected,
        })?;
        Ok((
            value,
            Wrapped {
                inner: variant,
                source: self.source,
                expected: self.expected,
            },
        ))
    }
}
impl<'de, A: de::VariantAccess<'de>> de::VariantAccess<'de> for Wrapped<'_, A> {
    type Error = A::Error;
    fn unit_variant(self) -> Result<(), Self::Error> {
        self.inner.unit_variant()
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        self.inner.newtype_variant_seed(Wrapped {
            inner: seed,
            source: self.source,
            expected: self.expected,
        })
    }
    fn tuple_variant<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.tuple_variant(
            len,
            Wrapped {
                inner: visitor,
                source: self.source,
                expected: self.expected,
            },
        )
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.inner.struct_variant(
            fields,
            Wrapped {
                inner: visitor,
                source: self.source,
                expected: self.expected,
            },
        )
    }
}

// Scalar inference preserves quoted strings. Containers are consumed only to
// produce a positioned type error; no integer field can retain their contents.
enum Scalar {
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
    Unit,
    Seq,
    Map,
}
impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ScalarVisitor;
        impl<'de> Visitor<'de> for ScalarVisitor {
            type Value = Scalar;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a scalar")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Scalar, E> {
                Ok(Scalar::Bool(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Scalar, E> {
                Ok(Scalar::Signed(v))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Scalar, E> {
                Ok(Scalar::Unsigned(v))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Scalar, E> {
                Ok(Scalar::Float(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Scalar, E> {
                Ok(Scalar::String(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Scalar, E> {
                Ok(Scalar::Unit)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Scalar, A::Error> {
                while a.next_element::<de::IgnoredAny>()?.is_some() {}
                Ok(Scalar::Seq)
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Scalar, A::Error> {
                while a.next_entry::<de::IgnoredAny, de::IgnoredAny>()?.is_some() {}
                Ok(Scalar::Map)
            }
        }
        d.deserialize_any(ScalarVisitor)
    }
}
impl Scalar {
    fn visit<'de, V: Visitor<'de>, E: de::Error>(self, v: V) -> Result<V::Value, E> {
        match self {
            Self::Bool(x) => v.visit_bool(x),
            Self::Signed(x) => v.visit_i64(x),
            Self::Unsigned(x) => v.visit_u64(x),
            Self::Float(x) => v.visit_f64(x),
            Self::String(x) => v.visit_string(x),
            Self::Unit => v.visit_unit(),
            Self::Seq => Err(E::invalid_type(de::Unexpected::Seq, &v)),
            Self::Map => Err(E::invalid_type(de::Unexpected::Map, &v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use std::path::Path;

    #[derive(Debug, Deserialize, PartialEq)]
    enum Payload {
        Number(u16),
        Pair(u16, u8),
        Named { value: u16 },
    }

    #[test]
    fn enum_payloads_keep_integer_checks_in_every_variant_shape() {
        for (yaml, expected) in [
            ("Number: 4", Payload::Number(4)),
            ("Pair: [4, 2]", Payload::Pair(4, 2)),
            ("Named: {value: 4}", Payload::Named { value: 4 }),
        ] {
            assert_eq!(
                super::super::decode::<Payload>(Path::new("enum.yaml"), yaml).unwrap(),
                expected
            );
        }
        for yaml in [
            "Number: '4'",
            "Pair: ['4', 2]",
            "Pair: [4, '2']",
            "Named: {value: '4'}",
        ] {
            let error = super::super::decode::<Payload>(Path::new("enum.yaml"), yaml).unwrap_err();
            assert!(error.message().contains("invalid type: string"), "{error}");
        }
    }
}
