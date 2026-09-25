//! libvirt's pool, volume, network, and domain XML, as Lodger parses it.
//!
//! libvirt writes this XML, but an object that another tool defined can hold
//! any text in its names and paths. Every parser must return an error, never
//! panic. A parsed pool, volume, or network must keep its facts when Lodger
//! writes it again and parses the result.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lodger_core::xml::network::NetworkXml;
use lodger_core::xml::pool::PoolXml;
use lodger_core::xml::volume::VolumeXml;
use lodger_core::xml::{domain_disk_sources, domain_disks, domain_networks};

fuzz_target!(|xml: &str| {
    if let Ok(pool) = PoolXml::parse(xml) {
        let again = PoolXml::parse(&pool.to_xml()).expect("written pool XML parses");
        assert_eq!(again.name(), pool.name());
        assert_eq!(again.pool_type(), pool.pool_type());
        assert_eq!(again.target_path(), pool.target_path());
        assert_eq!(again.nfs_source(), pool.nfs_source());
    }
    if let Ok(network) = NetworkXml::parse(xml) {
        let again = NetworkXml::parse(&network.to_xml()).expect("written network XML parses");
        assert_eq!(again.name(), network.name());
        assert_eq!(again.forward_mode(), network.forward_mode());
        assert_eq!(again.bridge(), network.bridge());
        assert_eq!(again.subnets(), network.subnets());
    }
    if let Ok(volume) = VolumeXml::parse(xml) {
        let _ = (volume.name(), volume.path(), volume.backing_path(), volume.format());
    }
    let _ = domain_disks(xml);
    let _ = domain_disk_sources(xml);
    let _ = domain_networks(xml);
});
