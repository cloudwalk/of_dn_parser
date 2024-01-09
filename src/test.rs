use std::str::FromStr;

use pretty_assertions::assert_eq;

use crate::{DistinguishedName, RelativeDistinguishedName, RelativeDistinguishedNameType};

#[test]
fn parse_empty_dn() {
    let dn = DistinguishedName::from_str("").unwrap();

    assert_eq!(dn, DistinguishedName { rdns: Vec::new() });
}

#[test]
fn parse_dn() {
    let dn = DistinguishedName::from_str("CN=web.conftpp.directory.openbankingbrasil.org.br,UID=bc97b8f0-cae0-4f2f-9978-d93f0e56a833,2.5.4.97=#0c2a4f464242522d64373338346264302d383432662d343363352d626530322d396432623264356566633263,L=SAO PAULO,ST=SP,O=Chicago Advisory Partners,C=BR,2.5.4.5=#130e3433313432363636303030313937,1.3.6.1.4.1.311.60.2.1.3=#13024252,2.5.4.15=#0c1450726976617465204f7267616e697a6174696f6e").unwrap();

    assert_eq!(
        dn,
        DistinguishedName {
            rdns: vec![
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::BusinessCategory,
                    value: "private organization".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::JurisdictionCountryName,
                    value: "BR".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::SerialNumber,
                    value: "43142666000197".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::C,
                    value: "BR".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::O,
                    value: "Chicago Advisory Partners".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::St,
                    value: "SP".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::L,
                    value: "SAO PAULO".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::OrganizationIdentifier,
                    value: "ofbbr-d7384bd0-842f-43c5-be02-9d2b2d5efc2c".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::Uid,
                    value: "bc97b8f0-cae0-4f2f-9978-d93f0e56a833".to_owned()
                }),
                (RelativeDistinguishedName {
                    ty: RelativeDistinguishedNameType::Cn,
                    value: "web.conftpp.directory.openbankingbrasil.org.br".to_owned()
                }),
            ]
        }
    );
}
