//! Distinguished name (DN) parser and formatter following OpenFinance
//! Brasil's DCR 1.0 standard.

use std::{
    result,
    str::{self, FromStr, Utf8Error},
    string::FromUtf8Error,
};

use derive_more::{Display, Error, From};

#[cfg(test)]
mod test;

// List of symbols that must be escaped with a backslash
const ESCAPABLE_SYMBOLS: [char; 10] = [' ', '"', '#', '+', ',', ';', '<', '=', '>', '\\'];

/// Possible errors when parsing distinguished names.
#[derive(Debug, Display, Error, From)]
pub enum Error {
    /// Could not decode a hex string.
    Hex(hex::FromHexError),
    /// Found an invalid RDN type.
    #[display(fmt = "invalid RDN type: {_0}")]
    #[from(ignore)]
    InvalidType(#[error(not(source))] String),
    /// Found an invalid value for the specified RDN type.
    #[display(fmt = "invalid value for {ty:?}: {value}")]
    #[from(ignore)]
    InvalidValue { ty: RdnType, value: String },
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
    FromUtf8(FromUtf8Error),
    /// Found a non-UTF-8 string.
    Utf8(Utf8Error),
}

/// Parsing result type.
pub type Result<T> = result::Result<T, Error>;

/// A distinguished name (DN).
///
/// DNs are composed of a sequence of key-value pairs called relative
/// distinguished names (RDNs).
#[derive(Clone, Debug)]
pub struct DistinguishedName {
    rdns: Vec<RelativeDistinguishedName>,
}

impl DistinguishedName {
    /// Find the value of the first occurence of the given RDN type.
    pub fn find(&self, ty: RdnType) -> Option<&str> {
        self.rdns
            .iter()
            .find_map(|x| if x.ty() == ty { Some(x.value()) } else { None })
    }

    /// Returns an iterator over all RDNs of this DN.
    pub fn iter(&self) -> impl Iterator<Item = &RelativeDistinguishedName> {
        self.rdns.iter()
    }

    /// Create a comparator for this DN.
    /// [RFC4518](https://datatracker.ietf.org/doc/html/rfc451) requires that
    /// DNs be transformed before comparison, which is implemented by this
    /// comparator.
    pub fn comparator(&self) -> Result<DnComparator> {
        DnComparator::new(self)
    }

    /// Serialize into the OpenFinance variant string format:
    /// <https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240649661/EN+Open+Finance+Brasil+Financial-grade+API+Dynamic+Client+Registration+1.0+Implementers+Draft+3#7.1.2.-Certificate-Distinguished-Name-Parsing>.
    pub fn to_of_string(&self) -> String {
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
                res.reserve(value.len());
                for c in value.chars() {
                    if ESCAPABLE_SYMBOLS.contains(&c) {
                        // Note: for simplicity we'll be escaping everything
                        // we can unconditionally even when this is not
                        // necesary
                        res.push('\\');
                    }
                    res.push(c);
                }
            }
        }

        res
    }
}

/// Parse from the canonical string format:
/// <https://datatracker.ietf.org/doc/html/rfc4514>.
impl FromStr for DistinguishedName {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // This format is faily straightforward and so the parser is
        // implemented manually. Parser crates wouldn't help by much.
        let mut rdns = Vec::new();
        let mut acc = Vec::new();
        let mut escaping = Escaping::None;
        let mut value_is_hex = false;
        let mut ty = None::<RdnType>;
        let chars = s.bytes().map(ParseItem::from).chain([ParseItem::Eof]);
        for c in chars {
            if escaping.is_pending() {
                let ParseItem::Byte(c) = c else {
                    // Cannot end a DN with a backslash
                    return Err(Error::UnexpectedEof);
                };
                if let Some(escaped) = escaping.consume(c)? {
                    acc.push(escaped);
                }

                continue;
            }

            match c {
                // A DN is a list of RDNs separated by commas
                ParseItem::Byte(b',') | ParseItem::Eof => {
                    let value = str::from_utf8(&acc)?.trim();
                    if value.is_empty() {
                        if c.is_eof() && ty.is_none() {
                            // EOF and the DN is complete
                            break;
                        } else {
                            // We already parsed a type but this RDN is
                            // missing a value
                            return if c.is_eof() {
                                Err(Error::UnexpectedEof)
                            } else {
                                Err(Error::UnexpectedCharacter(','))
                            };
                        }
                    }

                    // If we're ending the definition of this RDN then we must
                    // already have parsed an RDN type
                    let rdn_type = ty.ok_or_else(|| {
                        if c.is_eof() {
                            Error::UnexpectedEof
                        } else {
                            Error::UnexpectedCharacter(',')
                        }
                    })?;
                    ty = None;

                    // Decode the value. This may be a hex encoded string
                    let rdn_value = if value_is_hex {
                        value_is_hex = false;
                        let value = hex::decode(value)?;

                        String::from_utf8(value)?
                    } else {
                        value.to_owned()
                    };
                    acc.clear();

                    rdns.push(RelativeDistinguishedName::new(rdn_type, rdn_value));
                }
                // An RDN is an RDN type and a value separated by an equals
                // sign
                ParseItem::Byte(b'=') => {
                    if ty.is_some() {
                        // Something like 'a = b = c' is not a valid RDN
                        return Err(Error::UnexpectedCharacter('='));
                    }

                    let ty_str = str::from_utf8(&acc)?.trim();
                    if ty_str.is_empty() {
                        return Err(Error::UnexpectedCharacter('='));
                    }

                    ty = Some(ty_str.parse()?);
                    acc.clear();
                }
                // A backslash starts an escape sequence
                ParseItem::Byte(b'\\') => {
                    escaping = Escaping::Started;
                }
                // An octothorpe right after the equals sign means that the
                // value is an encoded hex string
                ParseItem::Byte(b'#') => {
                    if acc.is_empty() {
                        value_is_hex = true;
                    } else {
                        acc.push(b'#');
                    }
                }
                // A plus sign is used to define multi-valued RDNs but we have
                // no need for this here
                ParseItem::Byte(b'+') => return Err(Error::UnsupportedMultiValueRdns),
                // Every other byte is a literal
                ParseItem::Byte(c) => acc.push(c),
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
    Byte(u8),
    Eof,
}

impl ParseItem {
    fn is_eof(self) -> bool {
        matches!(self, Self::Eof)
    }
}

impl From<u8> for ParseItem {
    fn from(value: u8) -> Self {
        Self::Byte(value)
    }
}

#[derive(Clone, Copy)]
enum Escaping {
    None,
    Started,
    Hex(u8),
}

impl Escaping {
    fn is_pending(self) -> bool {
        matches!(self, Self::Started | Self::Hex(_))
    }

    fn consume(&mut self, c: u8) -> Result<Option<u8>> {
        match *self {
            Self::Started => {
                if ESCAPABLE_SYMBOLS.contains(&(c as char)) {
                    *self = Self::None;

                    Ok(Some(c))
                } else {
                    *self = Self::Hex(c);

                    Ok(None)
                }
            }
            Self::Hex(previous) => {
                *self = Self::None;
                let mut byte = [0; 1];
                hex::decode_to_slice([previous, c], &mut byte)?;

                Ok(Some(byte[0]))
            }
            Self::None => {
                unreachable!("BUG: called `Escaping::consume` when no escaping is active")
            }
        }
    }
}

/// A transformed [DistinguishedName] suitable for comparisons.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct DnComparator {
    rdns: Vec<RdnComparator>,
}

impl DnComparator {
    /// Create a new comparator from a [DistinguishedName].
    pub fn new(dn: &DistinguishedName) -> Result<Self> {
        let rdns = dn.iter().map(RdnComparator::new).collect::<Result<_>>()?;

        Ok(Self { rdns })
    }
}

/// A key-value pair that is part of a [DistinguishedName].
///
/// Multi-value RDNs are not supported.
#[derive(Clone, Debug)]
pub struct RelativeDistinguishedName {
    ty: RdnType,
    value: String,
}

impl RelativeDistinguishedName {
    /// Create a new RDN.
    pub fn new(ty: RdnType, value: String) -> Self {
        Self { ty, value }
    }

    /// Get the type of this RDN.
    pub fn ty(&self) -> RdnType {
        self.ty
    }

    /// Get the value of this RDN.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A transformed [RelativeDistinguishedName] suitable for comparisons.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct RdnComparator {
    ty: RdnType,
    value: String,
}

impl RdnComparator {
    /// Create a new comparator from a [RelativeDistinguishedName].
    pub fn new(rdn: &RelativeDistinguishedName) -> Result<Self> {
        let ty = rdn.ty();

        // Prepare the value so it can be compared correctly. Comparison
        // between values is fuzzy. Some characters must be replaced before
        // comparison, while others must be removed.
        //
        // <https://datatracker.ietf.org/doc/html/rfc4518#section-2>
        //
        // TODO: this is not 100% complete.
        let mut value = rdn
            .value()
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
                    Some(Err(Error::UnexpectedCharacter(c)))
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
            .collect::<Result<String>>()?;
        if !ty.is_comparison_case_sensitive() {
            value.make_ascii_lowercase();
        }

        // Clean the value of `organizationIdentifier` according to the OF
        // spec.
        //
        // One day the people working on the OpenFinance spec woke up with the
        // most brilliant idea ever: how about we add extra arbitrary
        // complexity for absolutely no reason at all? 'Genius!' they thought.
        // And so in their infinite wisdom they added the following:
        //
        // [...] convert ASN.1 values from OID 2.5.4.97 organizationIdentifier
        // to human readable text [...] retrieve the full value of the OID
        // 2.5.4.97 contained in the subject_DN. [...] Apply a filter using
        // regular expression to retrieve the org_id after ('OFBBR-')
        //
        // https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240649661/EN+Open+Finance+Brasil+Financial-grade+API+Dynamic+Client+Registration+1.0+Implementers+Draft+3#7.1.2.-Certificate-Distinguished-Name-Parsing
        //
        // That is, for `organizationIdentifier` ONLY, it is permissible to have
        // any amount of garbage before `OFBBR-`. Luckly this RDN is
        // case-insensitive so its value is lower case now and we don't need
        // an actual regex.
        if ty == RdnType::OrganizationIdentifier {
            let idx = value.find("ofbbr-").ok_or_else(|| Error::InvalidValue {
                ty: RdnType::OrganizationIdentifier,
                value: value.to_owned(),
            })?;
            value = value[idx..].to_owned();
        }

        Ok(Self {
            ty,
            value: value.trim().to_owned(),
        })
    }
}

/// A relative distinguished name type.
///
/// This is the type of a single component of a full DN. We only support a
/// select set of RDN types:
///
/// > the Authorization Server shall accept only the AttributeTypes
/// > (descriptors) defined in the last paragraph of clause 3 RFC4514 in
/// > string format, it shall also accept in OID format, with their values in
/// > ASN.1, all the AttributeTypes defined in Distinguished Name Open Finance
/// > Brasil x.509 Certificate Standards or added by the Certificate
/// > Authority.
///
/// <https://openfinancebrasil.atlassian.net/wiki/spaces/OF/pages/240650099/EN+Padr+o+de+Certificados+Open+Finance+Brasil+2.0#5.2.2.1.-Open-Finance-Brasil-Attributes>
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum RdnType {
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

impl RdnType {
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

    fn is_comparison_case_sensitive(self) -> bool {
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
/// <https://datatracker.ietf.org/doc/html/rfc4514>.
impl FromStr for RdnType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
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
            _ => Err(Error::InvalidType(s.to_owned())),
        }
    }
}
