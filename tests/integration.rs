#[cfg(test)]
mod elastic_test {
    use usiem::chrono;
    use std::thread;
    use usiem::components::command::{SiemCommandCall, SiemCommandHeader};
    use usiem::components::common::SiemMessage;
    use usiem::components::SiemComponent;
    use usiem::crossbeam_channel;
    use usiem::events::SiemLog;
    use usiem_elasticsearch::output::{ElasticOuputConfig, ElasticSearchOutput};

    #[test]
    fn test_elasticsearch_output() {
        let config = ElasticOuputConfig {
            commit_max_messages: 10,
            commit_timeout: 1,
            commit_time: 1,
            cache_size: 20,
            elastic_address: String::from("http://127.0.0.1:9200"),
            elastic_stream: String::from("log-integration-test"),
            bearer_token: None,
        };
        let (k_sender, _receiver) = crossbeam_channel::unbounded();
        let (log_sender_other, log_receiver) = crossbeam_channel::unbounded();
        let (log_sender, _log_receiver) = crossbeam_channel::unbounded();
        let mut es_output = ElasticSearchOutput::new(config);
        es_output.set_log_channel(log_sender, log_receiver);
        es_output.set_kernel_sender(k_sender);
        let component_channel = es_output.local_channel();

        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(50));

            for i in 0..100 {
                let log = SiemLog::new(
                    format!("Log for testing only {}", i),
                    chrono::Utc::now().timestamp_millis(),
                    "123.45.67.89",
                );
                log_sender_other.send(log).unwrap();
                println!("Sended log {}", i);
            }
            thread::sleep(std::time::Duration::from_millis(1000));
            component_channel
                .send(SiemMessage::Command(
                    SiemCommandHeader {
                        comm_id: 0,
                        comp_id: 0,
                        user: String::from("None"),
                    },
                    SiemCommandCall::STOP_COMPONENT("Component for testing".to_string()),
                ))
                .unwrap();
            println!("Ended sending logs");
        });
        es_output.run();
        thread::sleep(std::time::Duration::from_millis(100));
        println!("End");
    }
}
