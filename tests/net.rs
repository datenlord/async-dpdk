/// Test socket APIs.

#[cfg(test)]
mod net {
    use async_dpdk::{eal, net_dev, udp::UdpSocket};
    use tokio::task;

    mod test_single_client {
        use super::*;
        use std::net::{IpAddr, SocketAddr};

        const MSG: &'static str = "this is client message";
        const ACK: &'static str = "this is ack message";

        async fn server() {
            let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
            let mut buffer = [0u8; 30];
            let (sz, client_addr) = socket.recv_from(&mut buffer).await.unwrap();
            assert_eq!(sz, MSG.len());
            assert_eq!(client_addr.ip(), IpAddr::from([10, 2, 3, 1]));
            assert_eq!(&buffer[..sz], MSG.as_bytes());
            let sz = socket.send_to(ACK.as_bytes(), client_addr).await.unwrap();
            assert_eq!(sz, ACK.len());
        }

        async fn client() {
            let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
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
            let _ = eal::Config::new()
                .device_probe(&["10.2.3.0", "10.2.3.1"])
                .enter();
            net_dev::device_start().unwrap();
            let server = task::spawn(server());
            client().await;
            server.await.unwrap();
            net_dev::device_stop().unwrap();
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
            let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
            let mut buffer = [0u8; 30];
            let _sz = socket
                .send_to(msg.as_bytes(), "10.2.3.0:1234")
                .await
                .unwrap();
            let _ = socket.recv_from(&mut buffer).await.unwrap();
        }

        #[tokio::test]
        async fn test() {
            let _ = eal::Config::new()
                .device_probe(&["10.2.3.0", "10.2.3.1"])
                .enter();
            net_dev::device_start().unwrap();
            let server = task::spawn(server());
            client(0).await;
            client(1).await;
            server.await.unwrap();
            net_dev::device_stop().unwrap();
        }
    }

    #[cfg(test)]
    mod test_fragementation {
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
            let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
            let buffer = [1u8; LEN];
            let sz = socket.send_to(&buffer[..], "10.2.3.0:1234").await.unwrap();
            assert_eq!(sz, LEN);
        }

        #[tokio::test]
        async fn test() {
            let _ = eal::Config::new()
                .device_probe(&["10.2.3.0", "10.2.3.1"])
                .enter();
            net_dev::device_start().unwrap();
            let server = task::spawn(server());
            client().await;
            server.await.unwrap();
            net_dev::device_stop().unwrap();
        }
    }
}
