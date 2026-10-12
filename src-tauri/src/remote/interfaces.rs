//! The addresses this PC can be reached at and its LAN adapter's hardware address, for the pairing code.

use std::net::Ipv4Addr;

/// One network adapter that is up.
pub(super) struct Interface {
    pub(super) addresses: Vec<Ipv4Addr>,
    pub(super) mac: Option<[u8; 6]>,
}

/// Where a phone can reach this PC, the LAN address first, and the MAC of the adapter holding it.
pub(super) fn reachable(lan: Option<Ipv4Addr>) -> (Vec<Ipv4Addr>, Option<[u8; 6]>) {
    let adapters = adapters();
    let mac = lan.and_then(|lan| {
        adapters
            .iter()
            .find(|adapter| adapter.addresses.contains(&lan))
            .and_then(|adapter| adapter.mac)
    });

    let mut addresses: Vec<Ipv4Addr> = lan.into_iter().collect();
    for address in adapters.iter().flat_map(|adapter| &adapter.addresses) {
        if offered(*address) && !addresses.contains(address) {
            addresses.push(*address);
        }
    }

    (addresses, mac)
}

/// Private and Tailscale addresses, which the gateway's certificate authority may certify; never loopback or link-local.
fn offered(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    let tailscale = first == 100 && (64..128).contains(&second);

    address.is_private() || tailscale
}

#[cfg(windows)]
fn adapters() -> Vec<Interface> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    const BUFFER_OVERFLOW: u32 = 111;
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

    // u64 elements keep the buffer aligned for the structures written into it.
    let mut buffer: Vec<u64> = vec![0; 2048];
    let mut size = (buffer.len() * 8) as u32;
    let filled = loop {
        // SAFETY: buffer holds `size` writable bytes; the call writes the adapter list into it or asks for more.
        let result = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_INET),
                flags,
                std::ptr::null(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match result {
            0 => break true,
            BUFFER_OVERFLOW => buffer.resize((size as usize).div_ceil(8), 0),
            _ => break false,
        }
    };
    if !filled {
        return Vec::new();
    }

    let mut found = Vec::new();
    let mut adapter = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    // SAFETY: the list and each unicast chain are null-terminated and live in `buffer`, which outlives the walk;
    // an AF_INET query yields only IPv4 socket addresses.
    unsafe {
        while let Some(current) = adapter.as_ref() {
            if current.OperStatus == IfOperStatusUp {
                let mac = (current.PhysicalAddressLength == 6)
                    .then(|| std::array::from_fn(|index| current.PhysicalAddress[index]));
                let mut addresses = Vec::new();
                let mut unicast = current.FirstUnicastAddress;
                while let Some(entry) = unicast.as_ref() {
                    let socket = entry.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                    if let Some(socket) = socket.as_ref() {
                        addresses.push(Ipv4Addr::from(socket.sin_addr.S_un.S_addr.to_ne_bytes()));
                    }
                    unicast = entry.Next;
                }
                found.push(Interface { addresses, mac });
            }
            adapter = current.Next;
        }
    }

    found
}

/// Addresses only: reading hardware addresses differs per unix, and Wake-on-LAN is offered from Windows first.
#[cfg(unix)]
fn adapters() -> Vec<Interface> {
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs allocates the list it hands back, which is freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Vec::new();
    }

    let mut addresses = Vec::new();
    let mut entry = list;
    // SAFETY: the list is null-terminated and lives until freeifaddrs; AF_INET entries hold a sockaddr_in.
    unsafe {
        while let Some(current) = entry.as_ref() {
            let up = current.ifa_flags & libc::IFF_UP as libc::c_uint != 0;
            if up
                && let Some(address) = current.ifa_addr.as_ref()
                && i32::from(address.sa_family) == libc::AF_INET
            {
                let socket = &*current.ifa_addr.cast::<libc::sockaddr_in>();
                addresses.push(Ipv4Addr::from(u32::from_be(socket.sin_addr.s_addr)));
            }
            entry = current.ifa_next;
        }
        libc::freeifaddrs(list);
    }

    vec![Interface { addresses, mac: None }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_private_and_tailscale_addresses_are_offered() {
        for address in ["192.168.1.20", "10.0.0.5", "172.20.1.1", "100.101.2.3"] {
            assert!(offered(address.parse().unwrap()), "{address}");
        }
        for address in ["127.0.0.1", "169.254.3.4", "8.8.8.8", "100.200.1.1"] {
            assert!(!offered(address.parse().unwrap()), "{address}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn this_machines_adapters_list_its_lan_address_first_with_a_mac() {
        let lan = super::super::transport::local_ipv4();
        let (addresses, mac) = reachable(lan);

        if let Some(lan) = lan {
            assert_eq!(addresses.first(), Some(&lan));
            assert!(
                mac.is_some(),
                "the adapter holding the LAN address has a hardware address"
            );
        }
    }
}
