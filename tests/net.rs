/// Test socket APIs.
use async_dpdk::{
    eal::{self, *},
    net_dev,
    udp::UdpSocket,
};
use std::{env, sync::Once, time::Duration};
use tokio::{task, time};

static SETUP: Once = Once::new();

fn dpdk_setup() {
    SETUP.call_once(|| {
        env_logger::init();
        println!("{:?}", env::var("RUST_LOG"));
        eal::Config::new()
            .no_hugepages(true)
            .no_pci(true)
            .vdev(Vdev::Ring(0))
            .max_queues(1)
            .device_probe(&["10.2.3.0"])
            .unwrap()
            .enter()
            .unwrap();
        log::warn!("iniiniiniini");
    })
}

mod test_single_client {
    use super::*;
    use std::net::{IpAddr, SocketAddr};

    const MSG: &str = "this is client message";
    const ACK: &str = "this is ack message";

    async fn server() {
        let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
        let mut buffer = [0u8; 30];
        let (sz, client_addr) = socket.recv_from(&mut buffer).await.unwrap();
        assert_eq!(sz, MSG.len());
        assert_eq!(client_addr.ip(), IpAddr::from([10, 2, 3, 0]));
        assert_eq!(&buffer[..sz], MSG.as_bytes());
        let sz = socket.send_to(ACK.as_bytes(), client_addr).await.unwrap();
        assert_eq!(sz, ACK.len());
    }

    async fn client() {
        let socket = UdpSocket::bind("10.2.3.0:0").unwrap();
        let mut buffer = [0u8; 30];
        let sz = socket
            .send_to(MSG.as_bytes(), "10.2.3.0:1234")
            .await
            .unwrap();
        assert_eq!(sz, MSG.len());
        let (sz, server_addr) = socket.recv_from(&mut buffer).await.unwrap();
        assert_eq!(sz, ACK.len());
        assert_eq!(server_addr, SocketAddr::from(([10, 2, 3, 0], 1234)));
        assert_eq!(&buffer[..sz], ACK.as_bytes());
    }

    #[tokio::test]
    async fn test() {
        dpdk_setup();
        net_dev::device_start_all().unwrap();
        let server = task::spawn(server());
        time::sleep(Duration::from_millis(5)).await;
        client().await;
        server.await.unwrap();
        net_dev::device_stop_all().unwrap();
    }
}

#[cfg(test)]
mod test_multi_clients {
    use super::*;

    async fn server() {
        let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
        let mut buffer = [0u8; 30];
        for _ in 0..2 {
            let (sz, addr) = socket.recv_from(&mut buffer).await.unwrap();
            let _sz = socket.send_to(&buffer[..sz], addr).await.unwrap();
        }
    }

    async fn client(number: i32) {
        let msg = format!("my client number is {}", number);
        let socket = UdpSocket::bind("10.2.3.0:0").unwrap();
        let mut buffer = [0u8; 30];
        let _sz = socket
            .send_to(msg.as_bytes(), "10.2.3.0:1234")
            .await
            .unwrap();
        let _ = socket.recv_from(&mut buffer).await.unwrap();
    }

    #[tokio::test]
    async fn test() {
        dpdk_setup();
        net_dev::device_start_all().unwrap();
        let server = task::spawn(server());
        time::sleep(Duration::from_millis(5)).await;
        client(0).await;
        client(1).await;
        server.await.unwrap();
        net_dev::device_stop_all().unwrap();
    }
}

#[cfg(test)]
mod test_fragmentation {
    use super::*;

    const LEN: usize = 2000; // > Ethernet MTU

    async fn server() {
        let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
        let mut buffer = [0u8; LEN];
        let (sz, _addr) = socket.recv_from(&mut buffer).await.unwrap();
        assert_eq!(sz, LEN);
        assert_eq!(buffer[1], 1);
    }

    async fn client() {
        let socket = UdpSocket::bind("10.2.3.0:0").unwrap();
        let buffer = [1u8; LEN];
        let sz = socket.send_to(&buffer[..], "10.2.3.0:1234").await.unwrap();
        assert_eq!(sz, LEN);
    }

    #[tokio::test]
    async fn test() {
        dpdk_setup();
        net_dev::device_start_all().unwrap();
        let server = task::spawn(server());
        time::sleep(Duration::from_millis(5)).await;
        client().await;
        server.await.unwrap();
        net_dev::device_stop_all().unwrap();
    }
}
