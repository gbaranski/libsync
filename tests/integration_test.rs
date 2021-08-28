use libsync::delta::Delta;

mod common;

#[tokio::test]
async fn test_synchronize_state() {
    let (server, client) = common::setup().await;
    {
        let server = server.clone();
        tokio::spawn(async move { server.run().await.unwrap() });
    };
    {
        let client = client.clone();
        tokio::spawn(async move { client.run().await.unwrap() });
    };
    let deltas = &[
        Delta::Append(vec![1, 2, 3]), // [ 1, 2, 3 ]
        Delta::Reset,                 // [ ]
        Delta::Append(vec![4, 5]),    // [ 4, 5 ]
        Delta::Truncate(1),           // [ 4 ]
        Delta::Append(vec![6, 7, 8]), // [ 4, 6, 7, 8 ]
        Delta::Truncate(3),           // [ 4, 6, 7 ]
    ];

    for (i, delta) in deltas.into_iter().enumerate() {
        client.write(delta.clone()).await.unwrap();
        println!("wrote delta. i: {}", i);
        server.write_notify.notified().await;
    }

    assert_eq!(server.deltas.lock().await.resolve(), vec![4, 6, 7]);
}
