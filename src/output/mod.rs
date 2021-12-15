use chrono::prelude::*;
use crossbeam_channel::TryRecvError;
use crossbeam_channel::{Receiver, Sender};
use std::borrow::Cow;
use std::boxed::Box;
use std::time::{Duration, Instant};
use usiem::components::common::{
    CommandDefinition, SiemComponentCapabilities, SiemComponentStateStorage, SiemCommandCall,
    SiemFunctionType, SiemMessage, UserRole,
};
use usiem::components::SiemComponent;
use usiem::events::field::SiemField;
use usiem::events::SiemLog;

#[derive(Clone)]
pub struct ElasticOuputConfig {
    /// Max. messages for each HTTP request
    pub commit_max_messages: usize,
    pub cache_size: usize,
    /// Timeout for communication
    pub commit_timeout: u64,
    /// Send logs every X milliseconds
    pub commit_time: u64,
    /// ElasticSearch URI where it's listening
    pub elastic_address: String,
    /// ElasticSearch Data Stream to send logs to
    pub elastic_stream: String,
    pub bearer_token : Option<String>,
}

/// Basic SIEM component for sending logs to ElasticSearch
///
pub struct ElasticSearchOutput {
    comp_id : u64,
    /// Send actions to the kernel
    kernel_sender: Sender<SiemMessage>,
    /// Receive actions from other components or the kernel
    local_chnl_rcv: Receiver<SiemMessage>,
    /// Send actions to this components
    local_chnl_snd: Sender<SiemMessage>,
    /// Receive logs
    log_receiver: Receiver<SiemLog>,
    conn: Option<Box<dyn SiemComponentStateStorage>>,
    config: ElasticOuputConfig,
}
impl ElasticSearchOutput {
    pub fn new(config: ElasticOuputConfig) -> ElasticSearchOutput {
        let (kernel_sender, _receiver) = crossbeam_channel::bounded(1000);
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        let (_sndr, log_receiver) = crossbeam_channel::unbounded();
        return ElasticSearchOutput {
            comp_id : 0,
            kernel_sender,
            local_chnl_rcv,
            local_chnl_snd,
            log_receiver,
            config,
            conn: None,
        };
    }
    pub fn register_schema(&mut self) {
        //TODO: Store data schema to first create or update the StreamData in elasticsearch
    }
}
impl SiemComponent for ElasticSearchOutput {
    fn name(&self) -> &str {
        "ElasticSearchOutput"
    }
    fn local_channel(&self) -> Sender<SiemMessage> {
        self.local_chnl_snd.clone()
    }
    fn set_log_channel(&mut self, _sender: Sender<SiemLog>, receiver: Receiver<SiemLog>) {
        self.log_receiver = receiver;
    }
    fn set_kernel_sender(&mut self, sender: Sender<SiemMessage>) {
        self.kernel_sender = sender;
    }

    /// Execute the logic of this component in an infinite loop. Must be stopped using Commands sent using the channel.
    fn run(&mut self) {
        let receiver = self.local_chnl_rcv.clone();
        let log_receiver = self.log_receiver.clone();

        let client = reqwest::blocking::Client::new();
        let config = &self.config;

        let elastic_url = format!(
            "{}/{}/_bulk",
            &self.config.elastic_address[..],
            &self.config.elastic_stream[..]
        );
        let max_commit = self.config.commit_max_messages;
        let commit_time = Duration::from_millis(self.config.commit_time);
        let mut log_cache = Vec::with_capacity(self.config.cache_size);
        let mut last_commit = Instant::now();
        loop {
            loop {
                let rcv_action = receiver.try_recv();
                match rcv_action {
                    Ok(msg) => match msg {
                        SiemMessage::Command(_hdr,cmd) => match cmd {
                            SiemCommandCall::STOP_COMPONENT(_n) => return,
                            _ => {}
                        },
                        SiemMessage::Log(mut msg) => {
                            let time = format!("{:?}", Utc.timestamp_millis(msg.event_created()));
                            msg.add_field("@timestamp", SiemField::Text(Cow::Owned(time)));
                            log_cache.push(msg);
                            if log_cache.len() > max_commit || last_commit.elapsed() > commit_time {
                                // TODO: Use coarsetime
                                break;
                            }
                        }
                        _ => {}
                    },
                    Err(e) => match e {
                        TryRecvError::Empty => {
                            break;
                        }
                        TryRecvError::Disconnected => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    },
                }
            }
            let mut log_count = 0;
            if log_cache.len() < max_commit {
                loop {
                    let rcv_action = log_receiver.try_recv();
                    match rcv_action {
                        Ok(mut msg) => {
                            let time = format!("{:?}", Utc.timestamp_millis(msg.event_created()));
                            msg.add_field("@timestamp", SiemField::Text(Cow::Owned(time)));
                            log_cache.push(msg);
                            if log_cache.len() > max_commit || last_commit.elapsed() > commit_time {
                                // TODO: Use coarsetime
                                break;
                            }
                        }
                        Err(e) => {
                            match e {
                                TryRecvError::Empty => {
                                    //TODO: check time and quantity of logs
                                    break;
                                }
                                TryRecvError::Disconnected => {
                                    println!("Error???");
                                    return;
                                }
                            }
                        }
                    }
                    if log_count >= config.commit_max_messages {
                        break;
                    }
                    log_count += 1;
                }
            }
            let mut bulking: String = String::with_capacity(64_000);
            let mut err_cache = 0;
            for i in 0..max_commit {
                let msg = log_cache.get(i);
                match msg {
                    Some(msg) => {
                        err_cache = i;
                        let stringify = serde_json::to_string(&msg);
                        match stringify {
                            Ok(content) => {
                                let tmp = format!("{{\"create\":{{}}}}\n{}\n", content);
                                bulking.push_str(&tmp[..]);
                            }
                            Err(_) => {
                                //Can't happen
                            }
                        }
                    }
                    None => {
                        break;
                    }
                }
            }
            if err_cache > 0 {
                bulking.push_str(&(format!("\n"))[..]);
                let mut req = client.put(&elastic_url[..]).header("Content-Type", "application/json");
                match &config.bearer_token {
                    Some(tkn) => {
                        req = req.header("Authorization", format!("Bearer {}", tkn));
                    },
                    None => {}
                };
                match req .body(bulking).send()
                {
                    Ok(resp) => {
                        println!("PUT Logs");
                        println!("{:?}", resp.text().unwrap());
                        log_cache.drain(0..err_cache);
                        last_commit = Instant::now();
                    }
                    Err(err) => {
                        println!("{:?}", err);
                    }
                }
            }
        }
    }

    /// Allow to store information about this component like the state or conigurations.
    fn set_storage(&mut self, conn: Box<dyn SiemComponentStateStorage>) {
        self.conn = Some(conn);
    }

    /// Capabilities and actions that can be performed on this component
    fn capabilities(&self) -> SiemComponentCapabilities {
        let datasets = Vec::new();
        let mut commands = Vec::new();

        let stop_component = CommandDefinition::new(SiemFunctionType::STOP_COMPONENT,Cow::Borrowed("Stop ElasticSearch Output") ,Cow::Borrowed("This allows stopping all ElasticSearch components.\nUse only when really needed, like mantaining ElasticSearch.") , UserRole::Administrator);
        commands.push(stop_component);
        let start_component = CommandDefinition::new(
            SiemFunctionType::START_COMPONENT,// Must be added by default by the KERNEL and only used by him
            Cow::Borrowed("Start ElasticSearch Output"),
            Cow::Borrowed("This allows sending logs to ElasticSearch."),
            UserRole::Administrator,
        );
        commands.push(start_component);
        SiemComponentCapabilities::new(
            Cow::Borrowed("ElasticSearchOutput"),
            Cow::Borrowed("Send logs to elasticsearch"),
            Cow::Borrowed(""),
            datasets,
            commands,
            vec![]
        )
    }

    fn set_id(&mut self, id: u64) {
        self.comp_id = id;
    }

    fn duplicate(&self) -> Box<dyn SiemComponent> {
        let mut comp = ElasticSearchOutput::new(self.config.clone());
        comp.kernel_sender = self.kernel_sender.clone();
        comp.log_receiver = self.log_receiver.clone();
        comp.conn = self.conn.clone();
        Box::from(comp)
    }

    fn set_datasets(&mut self, _datasets : Vec<usiem::components::dataset::SiemDataset>) {
        // NO dataset needed
    }
}
