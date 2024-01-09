//! X509 certificate handling utiliies.

use std::{
    borrow::Cow,
    result,
    str::{self, FromStr},
    string::FromUtf8Error,
};

use derive_more::{Display, Error, From};
use once_cell::sync::Lazy;
use regex::{Regex, RegexBuilder};

#[cfg(test)]
mod test;

/// Possible errors when parsing distinguished names.
#[derive(Debug, Display, Error, From)]
pub enum ParseError {
    /// Could not decode a hex string.
    Hex(hex::FromHexError),
    /// Found an invalid RDN type.
    #[display(fmt = "invalid RDN type: {_0}")]
    #[from(ignore)]
    InvalidType(#[error(not(source))] String),
    /// Found an invalid value for the specified RDN type.
    #[display(fmt = "invalid value for {ty:?}: {value}")]
    #[from(ignore)]
    InvalidValue {
        ty: RelativeDistinguishedNameType,
        value: String,
    },
    /// Found a character in a position where it is invalid.
    #[display(fmt = "unexpected character: {_0:?}")]
    #[from(ignore)]
    UnexpectedCharacter(#[error(not(source))] char),
    /// String ended unexpectedly.
    #[display(fmt = "unexpected EOF")]
    UnexpectedEof,
    /// We don't support nor need to support multi-value RDNs.
    #[display(fmt = "multi-value RDNs are not supported")]
    UnsupportedMultiValueRdns,
    /// Found a non-UTF-8 string.
    Utf8(FromUtf8Error),
}

/// Parsing result type.
pub type ParseResult<T> = result::Result<T, ParseError>;

/// A distinguished name (DN).
///
/// DNs are composed of a sequence of key-value pairs called relative
/// distinguished names (RDNs).
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct DistinguishedName {
    rdns: Vec<RelativeDistinguishedName>,
}

impl DistinguishedName {
    /// Find the value of the first occurence of the given RDN type.
    pub fn find(&self, ty: RelativeDistinguishedNameType) -> Option<&str> {
        self.rdns
            .iter()
            .find_map(|x| if x.ty() == ty { Some(x.value()) } else { None })
    }

    // Prepare values so they can be compared correctly. Comparison between
    // DNs is fuzzy. Some characters must be replaced before comparison, while
    // others must be removed.
    //
    // <https://datatracker.ietf.org/doc/html/rfc4518#section-2>
    //
    // TODO: this is not 100% complete.
    fn value_for_comparison(is_case_sensitive: bool, value: &str) -> ParseResult<String> {
        let mut value = value
            .chars()
            .filter_map(|c| {
                if c == '\u{0340}'
                    || c == '\u{0341}'
                    || c == '\u{200E}'
                    || c == '\u{200F}'
                    || ('\u{202A}'..='\u{202E}').contains(&c)
                    || ('\u{206A}'..='\u{206F}').contains(&c)
                    || ('\u{E000}'..='\u{F8FF}').contains(&c)
                    || ('\u{F0000}'..='\u{FFFFD}').contains(&c)
                    || ('\u{100000}'..='\u{10FFFD}').contains(&c)
                    || c == '\u{FFFD}'
                {
                    // These characters are prohibited
                    Some(Err(ParseError::UnexpectedCharacter(c)))
                } else if c == '\u{0009}'
                    || c == '\u{000A}'
                    || c == '\u{000B}'
                    || c == '\u{000C}'
                    || c == '\u{000D}'
                    || c == '\u{0085}'
                    || c.is_whitespace()
                {
                    // These characters are compared as if they were a simple
                    // space
                    Some(Ok(' '))
                } else if c == '\u{00AD}'
                    || c == '\u{1806}'
                    || c == '\u{034F}'
                    || ('\u{180B}'..='\u{180D}').contains(&c)
                    || ('\u{FE0F}'..='\u{FF00}').contains(&c)
                    || c == '\u{FFFC}'
                    || c.is_control()
                    || c == '\u{200B}'
                {
                    // These characters are ignored during comparison
                    None
                } else {
                    // Character is used in comparisons
                    Some(Ok(c))
                }
            })
            .collect::<ParseResult<String>>()?;
        if !is_case_sensitive {
            value.make_ascii_lowercase();
        }
        value = value.trim().to_owned();

        Ok(value)
    }

    // Clean the value of `organizationIdentifier` according to the OF spec.
    //
    // One day the people working on the OpenFinance spec woke up with the
    // most brilliant idea ever: how about we add extra arbitrary complexity
    // for absolutely no reason at all? 'Genius!' they thought. And so in
    // their infinite wisdom they added the following:
    //
    // [...] convert ASN.1 values from OID 2.5.4.97 organizationIdentifier to
    // human readable text [...] retrieve the full value of the OID 2.5.4.97
    // contained in the subject_DN. [...] Apply a filter using regular
    // expression to retrieve the org_id after ('OFBBR-')
    //
    // <https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240649661/EN+Open+Finance+Brasil+Financial-grade+API+Dynamic+Client+Registration+1.0+Implementers+Draft+3#7.1.2.-Certificate-Distinguished-Name-Parsing>
    //
    // That is, for `organizationIdentifier` ONLY, it is permissible to have
    // any amount of garbage before `OFBBR-`. This RDN has also a
    // case-insensitive comparison, which means that we have to do a
    // case-insensitive search for `OFBBR-` as well.
    fn clean_organization_identifier(value: &str) -> ParseResult<String> {
        static OFBBR_REGEX: Lazy<Regex> = Lazy::new(|| {
            RegexBuilder::new("OFBBR-.*$")
                .case_insensitive(true)
                .build()
                .unwrap()
        });

        Ok(OFBBR_REGEX
            .find(value)
            .ok_or_else(|| ParseError::InvalidValue {
                ty: RelativeDistinguishedNameType::OrganizationIdentifier,
                value: value.to_owned(),
            })?
            .as_str()
            .to_owned())
    }
}

/// Serialize into the OpenFinance variant string format:
/// <https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240649661/EN+Open+Finance+Brasil+Financial-grade+API+Dynamic+Client+Registration+1.0+Implementers+Draft+3#7.1.2.-Certificate-Distinguished-Name-Parsing>.
impl ToString for DistinguishedName {
    fn to_string(&self) -> String {
        let mut res = String::new();
        for (i, rdn) in self.rdns.iter().rev().enumerate() {
            if i > 0 {
                res.push(',');
            }

            let ty = rdn.ty();
            let value = rdn.value();
            res += ty.as_of_str();
            res.push('=');
            if ty.of_encodes_as_hex() {
                res.push('#');
                res += &hex::encode(value);
            } else {
                res += value;
            }
        }

        res
    }
}

/// Parse from the canonical string format:
/// <https://datatracker.ietf.org/doc/html/rfc2253>.
///
/// We don't support additional LDAPv2-compatibility syntax.
impl FromStr for DistinguishedName {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        // This format is faily straightforward and so the parser is
        // implemented manually. Parser crates wouldn't help by much.
        let mut rdns = Vec::new();
        let mut acc = String::new();
        let mut is_escaped = false;
        let mut value_is_hex = false;
        let mut ty = None::<RelativeDistinguishedNameType>;
        let chars = s.chars().map(ParseItem::from).chain([ParseItem::Eof]);
        for c in chars {
            // TODO: escaping is more complex because you can escape a literal
            // byte too:
            // https://datatracker.ietf.org/doc/html/rfc2253#section-2.4
            if is_escaped {
                is_escaped = false;
                let ParseItem::Char(c) = c else {
                    // Cannot end a DN with a backslash
                    return Err(ParseError::UnexpectedEof);
                };
                acc.push(c);

                continue;
            }

            match c {
                // A DN is a list of RDNs separated by commas
                ParseItem::Char(',') | ParseItem::Eof => {
                    let value = acc.trim();
                    if value.is_empty() {
                        if c.is_eof() && ty.is_none() {
                            // EOF and the RDN is incomplete
                            break;
                        } else {
                            // We already parsed a type but this RDN is
                            // missing a value
                            return Err(ParseError::UnexpectedEof);
                        }
                    }

                    // If we're ending the definition of this RDN then we must
                    // already have parsed an RDN type
                    let rdn_type = ty.ok_or_else(|| {
                        if c.is_eof() {
                            ParseError::UnexpectedEof
                        } else {
                            ParseError::UnexpectedCharacter(',')
                        }
                    })?;
                    ty = None;

                    // Decode the value. This may be a hex encoded string
                    let mut value = if value_is_hex {
                        value_is_hex = false;
                        let value = hex::decode(value)?;

                        Cow::Owned(String::from_utf8(value)?)
                    } else {
                        value.into()
                    };

                    if rdn_type == RelativeDistinguishedNameType::OrganizationIdentifier {
                        value = Self::clean_organization_identifier(&value)?.into();
                    }

                    // Values must go through a preprocessing step before
                    // being compared. Perform this preprocessing here for
                    // simplicity
                    let rdn_value = Self::value_for_comparison(
                        rdn_type.comparison_is_case_sensitive(),
                        &value,
                    )?;
                    acc.clear();

                    rdns.push(RelativeDistinguishedName::new(rdn_type, rdn_value));
                }
                // An RDN is an RDN type and a value separated by an equals
                // sign
                ParseItem::Char('=') => {
                    if ty.is_some() {
                        // Something like 'a = b = c' is not a valid RDN
                        return Err(ParseError::UnexpectedCharacter('='));
                    }

                    ty = Some(acc.trim().parse()?);
                    acc.clear();
                }
                // A backslash starts an escape sequence
                ParseItem::Char('\\') => {
                    is_escaped = true;
                }
                // An octothorpe at the beginning of a value means that the
                // value is an encoded hex string
                ParseItem::Char('#') => {
                    let acc_is_empty = acc.trim().is_empty();
                    if acc_is_empty {
                        value_is_hex = true;
                        acc.clear();
                    } else {
                        acc.push('#');
                    }
                }
                // A plus sign is used to define multi-valued RDNs but we have
                // no need for this here
                ParseItem::Char('+') => return Err(ParseError::UnsupportedMultiValueRdns),
                // Every other character is a literal
                ParseItem::Char(c) => acc.push(c),
            }
        }

        // For some reason the string format serializes RDNs in the inverse
        // order
        rdns.reverse();

        Ok(Self { rdns })
    }
}

#[derive(Clone, Copy)]
enum ParseItem {
    Char(char),
    Eof,
}

impl ParseItem {
    fn is_eof(self) -> bool {
        matches!(self, Self::Eof)
    }
}

impl From<char> for ParseItem {
    fn from(value: char) -> Self {
        Self::Char(value)
    }
}

/// A key-value pair that is part of a DN.
///
/// Multi-value RDNs are not supported.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct RelativeDistinguishedName {
    ty: RelativeDistinguishedNameType,
    value: String,
}

impl RelativeDistinguishedName {
    /// Create a new RDN.
    pub fn new(ty: RelativeDistinguishedNameType, value: String) -> Self {
        Self { ty, value }
    }

    /// Get the type of this RDN.
    pub fn ty(&self) -> RelativeDistinguishedNameType {
        self.ty
    }

    /// Get the value of this RDN.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A relative distinguished name (RDN) type.
///
/// This is the type of a single component of a full DN. We only support a
/// select set of RDN types:
///
/// the Authorization Server shall accept only the AttributeTypes
/// (descriptors) defined in the last paragraph of clause 3 RFC4514 in string
/// format, it shall also accept in OID format, with their values in ASN.1,
/// all the AttributeTypes defined in Distinguished Name Open Finance Brasil
/// x.509 Certificate Standards or added by the Certificate Authority.
///
/// <https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240650099/EN+Padr+o+de+Certificados+Open+Finance+Brasil+2.0#5.2.2.1.-Open-Finance-Brasil-Attributes>
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelativeDistinguishedNameType {
    /// Common name.
    Cn,
    /// Locality name.
    L,
    /// State or province name.
    St,
    /// Organization name.
    O,
    /// Organizational unit name.
    Ou,
    /// Country name.
    C,
    /// Street address.
    Street,
    /// Domain component.
    Dc,
    /// User ID.
    Uid,
    /// Type of business category.
    BusinessCategory,
    /// Jurisdiction country name.
    JurisdictionCountryName,
    /// National Register of Legal Personnel (CNPJ) of the legal entity
    /// holding the certificate.
    SerialNumber,
    /// Participant Code associated with the CNPJ listed in the Directory
    /// Service of Open Finance Brasil.
    OrganizationIdentifier,
    /// Participant Code associated with the CNPJ listed in the Directory
    /// Service of Open Finance Brasil.
    OrganizationalUnitName,
}

impl RelativeDistinguishedNameType {
    fn as_of_str(self) -> &'static str {
        match self {
            Self::Cn => "CN",
            Self::L => "L",
            Self::St => "ST",
            Self::O => "O",
            Self::Ou => "OU",
            Self::C => "C",
            Self::Street => "Street",
            Self::Dc => "DC",
            Self::Uid => "UID",
            Self::BusinessCategory => "2.5.4.15",
            Self::JurisdictionCountryName => "1.3.6.1.4.1.311.60.2.1.3",
            Self::SerialNumber => "2.5.4.5",
            Self::OrganizationIdentifier => "2.5.4.97",
            Self::OrganizationalUnitName => "2.5.4.11",
        }
    }

    fn of_encodes_as_hex(self) -> bool {
        matches!(
            self,
            Self::BusinessCategory
                | Self::JurisdictionCountryName
                | Self::SerialNumber
                | Self::OrganizationIdentifier
                | Self::OrganizationalUnitName
        )
    }

    fn comparison_is_case_sensitive(self) -> bool {
        matches!(
            self,
            Self::Cn
                | Self::L
                | Self::St
                | Self::O
                | Self::Ou
                | Self::C
                | Self::JurisdictionCountryName
                | Self::OrganizationalUnitName
        )
    }
}

/// Parse from the canonical string format:
/// <https://datatracker.ietf.org/doc/html/rfc2253>.
impl FromStr for RelativeDistinguishedNameType {
    type Err = ParseError;

    fn from_str(s: &str) -> ParseResult<Self> {
        let lowercase_s = s.to_lowercase();

        match lowercase_s.strip_prefix("oid.").unwrap_or(&lowercase_s) {
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.3
            "cn" | "2.5.4.3" => Ok(Self::Cn),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.16
            "l" | "2.5.4.7" => Ok(Self::L),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.33
            "st" | "2.5.4.8" => Ok(Self::St),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.19
            "o" | "2.5.4.10" => Ok(Self::O),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.20
            "ou" => Ok(Self::Ou),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.2
            "c" | "2.5.4.6" => Ok(Self::C),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.34
            "street" | "2.5.4.9" => Ok(Self::Street),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.4
            "dc" | "0.9.2342.19200300.100.1.25" => Ok(Self::Dc),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.39
            "uid" | "0.9.2342.19200300.100.1.1" => Ok(Self::Uid),
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.1
            "businesscategory" | "2.5.4.15" => Ok(Self::BusinessCategory),
            // https://oidref.com/1.3.6.1.4.1.311.60.2.1.3
            "jurisdictioncountryname" | "1.3.6.1.4.1.311.60.2.1.3" => {
                Ok(Self::JurisdictionCountryName)
            }
            // https://datatracker.ietf.org/doc/html/rfc4519#section-2.31
            "serialnumber" | "2.5.4.5" => Ok(Self::SerialNumber),
            // https://oidref.com/2.5.4.97
            "organizationidentifier" | "2.5.4.97" => Ok(Self::OrganizationIdentifier),
            // https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240650099/EN+Padr+o+de+Certificados+Open+Finance+Brasil+2.0#5.2.2.1.-Open-Finance-Brasil-Attributes
            "organizationalunitname" | "2.5.4.11" => Ok(Self::OrganizationalUnitName),
            _ => Err(ParseError::InvalidType(s.to_owned())),
        }
    }
}
