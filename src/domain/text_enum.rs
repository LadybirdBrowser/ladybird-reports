/// Declares an enum that is stored and exchanged as one fixed piece of text.
///
/// Each variant names its text once. From that the macro derives the lookup in
/// both directions, `Display`, serde, and the PostgreSQL text mapping, so a
/// value can be bound to a query and read from a row without going through
/// `String`. An unknown text fails to decode instead of becoming a wrong state.
macro_rules! text_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = &'static str;

            fn from_str(value: &str) -> ::core::result::Result<Self, Self::Err> {
                Self::parse(value).ok_or(concat!("unknown ", stringify!($name)))
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> ::core::result::Result<Self, D::Error> {
                let text = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(
                    deserializer,
                )?;

                Self::parse(&text).ok_or_else(|| {
                    serde::de::Error::unknown_variant(&text, &[$($text),+])
                })
            }
        }

        impl sqlx::Type<sqlx::Postgres> for $name {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <str as sqlx::Type<sqlx::Postgres>>::type_info()
            }

            fn compatible(type_info: &sqlx::postgres::PgTypeInfo) -> bool {
                <str as sqlx::Type<sqlx::Postgres>>::compatible(type_info)
            }
        }

        impl sqlx::postgres::PgHasArrayType for $name {
            fn array_type_info() -> sqlx::postgres::PgTypeInfo {
                <&str as sqlx::postgres::PgHasArrayType>::array_type_info()
            }

            fn array_compatible(type_info: &sqlx::postgres::PgTypeInfo) -> bool {
                <&str as sqlx::postgres::PgHasArrayType>::array_compatible(type_info)
            }
        }

        impl sqlx::Encode<'_, sqlx::Postgres> for $name {
            fn encode_by_ref(
                &self,
                buffer: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> ::core::result::Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <&str as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.as_str(), buffer)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $name {
            fn decode(value: sqlx::postgres::PgValueRef<'r>) -> ::core::result::Result<Self, sqlx::error::BoxDynError> {
                let text = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;

                Self::parse(text)
                    .ok_or_else(|| format!("unknown {}: {text}", stringify!($name)).into())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    text_enum! {
        enum Sample {
            First => "first",
            SecondWord => "second_word",
        }
    }

    #[test]
    fn text_round_trips_in_both_directions() {
        for variant in Sample::ALL {
            assert_eq!(Sample::parse(variant.as_str()), Some(*variant));
            assert_eq!(variant.to_string(), variant.as_str());
        }

        assert_eq!(Sample::parse("unknown"), None);
        assert_eq!("second_word".parse::<Sample>(), Ok(Sample::SecondWord));
        assert!("Second_Word".parse::<Sample>().is_err());
    }

    #[test]
    fn serde_uses_the_text_and_rejects_unknown_values() {
        assert_eq!(
            serde_json::to_string(&Sample::SecondWord).unwrap(),
            "\"second_word\""
        );
        assert_eq!(
            serde_json::from_value::<Sample>(serde_json::json!("first")).unwrap(),
            Sample::First
        );
        assert!(serde_json::from_value::<Sample>(serde_json::json!("third")).is_err());
        assert!(serde_json::from_value::<Sample>(serde_json::json!(1)).is_err());
    }
}
