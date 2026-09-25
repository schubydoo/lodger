//! The builders that turn checked input into new pool, network, and volume XML.
//!
//! A pool path or an NFS export can hold `<`, `&`, or quotes. The builders
//! must escape them: the XML that a builder writes must parse, and it must
//! carry the same values back.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lodger_core::validate::Name;
use lodger_core::xml::network::{NetworkXml, NewNetwork};
use lodger_core::xml::pool::{NewPool, PoolSource, PoolXml};
use lodger_core::xml::volume::{NewVolume, VolumeFormat, VolumeXml};

fuzz_target!(|input: (String, String, String, String, u64)| {
    let (name, a, b, c, size) = input;
    let Ok(name) = Name::parse("Name", &name) else {
        return;
    };

    for pool in [
        NewPool::dir(name.clone(), &a),
        NewPool::nfs(name.clone(), &a, &b, &c),
    ]
    .into_iter()
    .flatten()
    {
        let parsed = PoolXml::parse(&pool.to_xml()).expect("a new pool's XML parses");
        assert_eq!(parsed.name(), Some(pool.name.as_str()));
        assert_eq!(parsed.target_path(), Some(pool.path.as_str()));
        match &pool.source {
            PoolSource::Dir => assert_eq!(parsed.pool_type(), Some("dir")),
            PoolSource::Nfs { host, export } => {
                assert_eq!(parsed.pool_type(), Some("netfs"));
                assert_eq!(parsed.nfs_source(), Some((host.as_str(), export.as_str())));
            }
        }
    }

    let bridges = vec![b.clone()];
    for network in [
        NewNetwork::nat(name.clone(), &a),
        NewNetwork::isolated(name.clone(), &a),
        NewNetwork::bridge(name.clone(), &b, &bridges),
    ]
    .into_iter()
    .flatten()
    {
        let parsed = NetworkXml::parse(&network.to_xml()).expect("a new network's XML parses");
        assert_eq!(parsed.name(), Some(network.name.as_str()));
        assert_eq!(parsed.subnets(), network.subnet().into_iter().collect::<Vec<_>>());
    }

    for format in [VolumeFormat::Qcow2, VolumeFormat::Raw] {
        if let Ok(volume) = NewVolume::new(name.clone(), format, size) {
            let parsed = VolumeXml::parse(&volume.to_xml()).expect("a new volume's XML parses");
            assert_eq!(parsed.name(), Some(volume.name.as_str()));
            assert_eq!(parsed.format(), Some(format.as_str()));
        }
    }
});
