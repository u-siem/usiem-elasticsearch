use usiem::components::SiemComponent;
use usiem::components::common::{CommandDefinition, SiemMessage, SiemComponentStateStorage, SiemComponentCapabilities, SiemFunctionType, UserRole, SiemFunctionCall};
use usiem::events::{SiemLog};
use crossbeam_channel::{Receiver, Sender};
use std::borrow::Cow;
use std::boxed::Box;
use crossbeam_channel::{TryRecvError};
use std::time::Duration;

pub struct ElasticOuputConfig {
    /// Max. messages for each HTTP request
    pub commit_max_messages : usize,
    /// Timeout for communication
    pub commit_timeout : i64,
    /// Send logs every X milliseconds
    pub commit_time : i64,
    /// ElasticSearch URI where it's listening
    pub elastic_address: String,
    /// ElasticSearch Data Stream to send logs to
    pub elastic_stream : String,
}

/// Basic SIEM component for sending logs to ElasticSearch
/// 
pub struct ElasticSearchOutput {
    /// Send actions to the kernel
    kernel_sender: Sender<SiemMessage>,
    /// Receive actions from other components or the kernel
    local_chnl_rcv: Receiver<SiemMessage>,
    /// Send actions to this components
    local_chnl_snd: Sender<SiemMessage>,
    /// Receive logs
    log_receiver: Receiver<SiemLog>,
    conn : Option<Box<dyn SiemComponentStateStorage>>,
    config : ElasticOuputConfig
}
impl ElasticSearchOutput {
    pub fn new(config : ElasticOuputConfig) -> ElasticSearchOutput {
        let (kernel_sender, _receiver) = crossbeam_channel::bounded(1000);
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        let (_sndr, log_receiver) = crossbeam_channel::unbounded();
        return ElasticSearchOutput {
            kernel_sender,
            local_chnl_rcv,
            local_chnl_snd,
            log_receiver,
            config,
            conn : None
        }
    }
}
impl SiemComponent for ElasticSearchOutput {
    fn name(&self) -> Cow<'static, str>{
        Cow::Borrowed("ElasticSearchOutput")
    }
    fn local_channel(&self) -> Sender<SiemMessage> {
        self.local_chnl_snd.clone()
    }
    fn set_log_channel(&mut self, _sender : Sender<SiemLog>, receiver : Receiver<SiemLog>) {
        self.log_receiver = receiver;
    }
    fn set_kernel_sender(&mut self, sender : Sender<SiemMessage>) {
        self.kernel_sender = sender;
    }

    /// Execute the logic of this component in an infinite loop. Must be stopped using Commands sent using the channel.
    fn run(&mut self) {
        let receiver = self.local_chnl_rcv.clone();
        let log_receiver = self.log_receiver.clone();

        let mut client = reqwest::blocking::Client::new();
        let stream_index = &self.config.elastic_stream[..];
        let config = &self.config;

        let elastic_url = format!("{}/_bulk",&self.config.elastic_address[..]);
        loop {
            loop {
                let rcv_action = receiver.try_recv();
                match rcv_action {
                    Ok(msg) => {
                        match msg {
                            SiemMessage::Command(cmd) => {
                                match cmd {
                                    SiemFunctionCall::STOP_COMPONENT(_n) =>  {
                                        return
                                    },
                                    _ => {}
                                }
                            },
                            _ => {}
                        }
                    },
                    Err(e) => {
                        match e {
                            TryRecvError::Empty => {
                                break;
                            },
                            TryRecvError::Disconnected => {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                }
            }
            let mut bulking : String = String::with_capacity(64_000);
            let mut log_count = 0;
            loop {
                let rcv_action = log_receiver.try_recv();
                match rcv_action {
                    Ok(msg) => {
                        let stringify = serde_json::to_string(&msg);
                        match stringify {
                            Ok(content) => {
                                bulking.push(format!("{{\"create\":{{\"_index\":\"{}\"}}}}\n{}\n",stream_index, content));
                            }
                            Err(_) => {}
                        }
                    },
                    Err(e) => {
                        break;
                    }
                }
                if log_count >= config.commit_max_messages {
                    break;
                }
            }
            bulking.push(format!("\n"));
            match client.post(&elastic_url[..]).body(bulking).send() {
                Ok(_) => {},
                Err(_err) => {
                    //TODO: retry policy
                }
            }
        }
    }

    /// Allow to store information about this component like the state or conigurations.
    fn set_storage(&mut self, conn : Box<dyn SiemComponentStateStorage>) {
        self.conn = Some(conn);
    }

    /// Capabilities and actions that can be performed by this component
    fn capabilities(&self) -> SiemComponentCapabilities {
        let datasets = Vec::new();
        let mut commands = Vec::new();

        let stop_component = CommandDefinition::new(SiemFunctionType::STOP_COMPONENT,Cow::Borrowed("Stop ElasticSearch Output") ,Cow::Borrowed("This allows stopping all ElasticSearch components.\nUse only when really needed, like mantaining ElasticSearch.") , UserRole::Administrator);
        commands.push(stop_component);
        let start_component = CommandDefinition::new(SiemFunctionType::START_COMPONENT,Cow::Borrowed("Start ElasticSearch Output") ,Cow::Borrowed("This allows sending logs to ElasticSearch.") , UserRole::Administrator);
        commands.push(start_component);
        SiemComponentCapabilities::new(Cow::Borrowed("ElasticSearchOutput"), Cow::Borrowed("Send logs to elasticsearch"), Cow::Borrowed(""), datasets, commands)
    }
}
