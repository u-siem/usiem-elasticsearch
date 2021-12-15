pub mod output;
pub mod input;

#[cfg(test)]
mod elastic_test {
    use super::output::{ElasticSearchOutput, ElasticOuputConfig};
    use super::input::ElasticSearchInput;
    use usiem::components::{SiemComponent};
    use usiem::components::common::{SiemMessage, SiemCommandCall, SiemCommandHeader};
    use usiem::events::{SiemLog, SiemEvent};
    use usiem::events::field::SiemIp;
    use std::thread;
    use std::borrow::Cow;

    #[test]
    fn test_elasticsearch_in_out(){
        let (k_sender,_receiver) = crossbeam_channel::unbounded();

        let (log_sender_for_in,log_rec_for_in) = crossbeam_channel::unbounded();
        let (log_sender_for_out,log_rec_for_out) = crossbeam_channel::unbounded();

        let mut es_in = ElasticSearchInput::new("127.0.0.1:9200".to_string(), 1);
        es_in.set_kernel_sender(k_sender.clone());
        es_in.set_log_channel(log_sender_for_in.clone(), log_rec_for_in.clone());

        let config = ElasticOuputConfig {
            commit_max_messages : 30,
            commit_timeout : 1,
            commit_time : 1,
            cache_size : 50,
            elastic_address : String::from("http://127.0.0.1:9200"),
            elastic_stream : String::from("log-integration-test"),
            bearer_token : None
        };
        
        let mut es_output = ElasticSearchOutput::new(config);
        es_output.set_log_channel(log_sender_for_out.clone(),log_rec_for_out);
        es_output.set_kernel_sender(k_sender);
        let out_channel = es_output.local_channel();
        let in_channel = es_in.local_channel();

        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(50));

            for i in 0..100 {
                let log = SiemLog::new(format!("Log for testing only {}",i), chrono::Utc::now().timestamp_millis(), SiemIp::from_ip_str("123.45.67.89").unwrap());
                log_sender_for_out.send(log).unwrap();
                println!("Sended log {}",i);
            }
            thread::sleep(std::time::Duration::from_millis(3000));
            out_channel.send(SiemMessage::Command(SiemCommandHeader{
                comm_id : 0,
                comp_id : 0,
                user : String::from("None")
            }, SiemCommandCall::STOP_COMPONENT(Cow::Borrowed("Component for testing")))).unwrap();
            in_channel.send(SiemMessage::Command(SiemCommandHeader{
                comm_id : 0,
                comp_id : 0,
                user : String::from("None")
            }, SiemCommandCall::STOP_COMPONENT(Cow::Borrowed("Component for testing")))).unwrap();
            println!("Ended sending logs");
        });
        thread::spawn(move || {
            es_in.run();
        });
        thread::sleep(std::time::Duration::from_millis(100));
        thread::spawn(move || {
            es_output.run();
        });
        thread::sleep(std::time::Duration::from_millis(3000));
        let mut log_n = 0;
        loop {
            match log_rec_for_in.try_recv() {
                Ok(log) => {
                    log_n +=1;
                    match log.event() {
                        SiemEvent::Json(value) => {
                            assert!(value.get("message").unwrap().as_str().unwrap().contains("Log for testing only"))
                        },
                        _ => {
                            panic!("Invalid log body")
                        }
                    }
                    
                },
                Err(_e) => break
            };
        }
        assert_eq!(log_n, 100);

    }
}
