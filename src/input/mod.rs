use chrono::prelude::*;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use std::borrow::Cow;
use std::boxed::Box;
use std::sync::Arc;
use usiem::components::common::{
    CommandDefinition, SiemCommandCall, SiemComponentCapabilities, SiemComponentStateStorage,
    SiemFunctionType, SiemMessage, UserRole,
};
use usiem::components::SiemComponent;
use usiem::events::field::{SiemField, SiemIp};
use usiem::events::{SiemEvent, SiemLog};
use tide::prelude::*;
use tide::Request;

pub struct ElasticSearchInput {
    comp_id: u64,
    /// Send actions to the kernel
    kernel_sender: Sender<SiemMessage>,
    /// Receive actions from other components or the kernel
    local_chnl_rcv: Receiver<SiemMessage>,
    /// Send actions to this components
    local_chnl_snd: Sender<SiemMessage>,
    /// Receive logs
    log_sender: Sender<SiemLog>,
    conn: Option<Box<dyn SiemComponentStateStorage>>,
    listen_address: String,
    n_threads : usize
}

impl SiemComponent for ElasticSearchInput {
    fn set_id(&mut self, id: u64) {
        self.comp_id = id;
    }

    fn local_channel(&self) -> Sender<SiemMessage> {
        self.local_chnl_snd.clone()
    }

    fn set_log_channel(&mut self, sender: Sender<SiemLog>, _receiver: Receiver<SiemLog>) {
        self.log_sender = sender;
    }

    fn set_kernel_sender(&mut self, sender: Sender<SiemMessage>) {
        self.kernel_sender = sender;
    }

    fn run(&mut self) {
        std::env::set_var("ASYNC_STD_THREAD_COUNT", self.n_threads.to_string());
        std::env::set_var("ASYNC_STD_THREAD_NAME", self.name());
        let receiver = self.local_chnl_rcv.clone();
        let state = State::new(self.log_sender.clone());
        let addr = self.listen_address.clone();
        let jn = async_std::task::spawn(async move {
            let _ = Self::tide_listening(state, &addr).await;
        });
        async_std::task::block_on(async {
            loop {
                let rcv_action = receiver.try_recv();
                match rcv_action {
                    Ok(msg) => match msg {
                        SiemMessage::Command(_hdr, cmd) => match cmd {
                            SiemCommandCall::STOP_COMPONENT(_n) => {
                                println!("Closing ElasticSearchInput");
                                return
                            },
                            _ => {}
                        },
                        _ => {}
                    },
                    Err(e) => match e {
                        TryRecvError::Empty => {
                        },
                        TryRecvError::Disconnected => {
                            return;
                        }
                    },
                }
                async_std::task::yield_now().await;
                async_std::task::sleep(std::time::Duration::from_millis(10)).await;
            }
        });
        let _ = jn.cancel();
        return
    }

    fn set_storage(&mut self, _conn: Box<dyn SiemComponentStateStorage>) {
        // Not used
    }

    fn capabilities(&self) -> SiemComponentCapabilities {
        let datasets = Vec::new();
        let mut commands = Vec::new();

        let stop_component = CommandDefinition::new(SiemFunctionType::STOP_COMPONENT,Cow::Borrowed("Stop ElasticSearch Input") ,Cow::Borrowed("This allows stopping all ElasticSearch components.\nUse only when really needed, like mantaining ElasticSearch.") , UserRole::Administrator);
        commands.push(stop_component);
        let start_component = CommandDefinition::new(
            SiemFunctionType::START_COMPONENT, // Must be added by default by the KERNEL and only used by him
            Cow::Borrowed("Start ElasticSearch Input"),
            Cow::Borrowed("This allows sending logs to ElasticSearch."),
            UserRole::Administrator,
        );
        commands.push(start_component);
        SiemComponentCapabilities::new(
            Cow::Borrowed("ElasticSearchInput"),
            Cow::Borrowed("Send logs to elasticsearch"),
            Cow::Borrowed(""),
            datasets,
            commands,
            vec![],
        )
    }

    fn name(&self) -> &str {
        &"ElasticSearchInput"
    }

    fn duplicate(&self) -> Box<dyn SiemComponent> {
        let mut comp = ElasticSearchInput::new(self.listen_address.clone(), self.n_threads);
        comp.kernel_sender = self.kernel_sender.clone();
        comp.log_sender = self.log_sender.clone();
        comp.conn = self.conn.clone();
        Box::from(comp)
    }

    fn set_datasets(&mut self, _datasets: Vec<usiem::components::dataset::SiemDataset>) {
        // TODO: datasets like who contact us or the deny list
    }
}

#[derive(Clone)]
struct State {
    pub sender: Arc<Sender<SiemLog>>,
}
impl State {
    fn new(sender: Sender<SiemLog>) -> Self {
        Self {
            sender: Arc::from(sender),
        }
    }
}

impl ElasticSearchInput {
    pub fn new(listen_address: String, threads : usize) -> ElasticSearchInput {
        let threads = if threads == 0 {1} else {threads};
        let (kernel_sender, _receiver) = crossbeam_channel::bounded(1000);
        let (local_chnl_snd, local_chnl_rcv) = crossbeam_channel::unbounded();
        let (log_sender, _log_receiver) = crossbeam_channel::unbounded();
        return ElasticSearchInput {
            comp_id: 0,
            kernel_sender,
            local_chnl_rcv,
            local_chnl_snd,
            log_sender,
            conn: None,
            listen_address,
            n_threads : threads
        };
    }

    async fn tide_listening(state: State, listening_address: &str) -> tide::Result<()> {
        let mut app = tide::with_state(state);
        app.at("/").get(es_metadata);
        app.at("/_license").post(es_license);
        app.at("/_xpack").post(es_xpack);
        app.at("/_ilm/policy/:policy").post(es_ilm_policy);
        app.at("/_cat/templates/:temp_id").post(es_cat_templates);
        app.at("/_bulk").post(es_bulk).put(es_bulk);
        app.at("/:stream/_bulk").post(es_bulk).put(es_bulk);
        app.listen(listening_address).await?;
        Ok(())
    }
}

fn invalid_parameter(val: String) -> tide::Result {
    let mut res = tide::Response::new(tide::StatusCode::BadRequest);
    res.set_body(tide::Body::from_string(val));
    tide::Result::Ok(res)
}

async fn es_bulk(mut req: Request<State>) -> tide::Result {
    let data = req.body_string().await?;
    let state = req.state();
    let stream = match req.param("stream") {
        Ok(stream) => stream.to_string(),
        Err(_) => "default".to_string(),
    };
    println!("Received bulk request from {} for {}", req.remote().unwrap_or("unknown"), stream);
    let utc: DateTime<Utc> = Utc::now();
    let mut logs_indexed = 0;
    let mut logs_failed = 0;
    let mut action: Option<serde_json::Value> = None;
    let mut n_line = 0;
    let mut to_ret = Vec::with_capacity(1024);
    let rng = fastrand::Rng::new();
    for line in data.lines() {
        let act: Option<serde_json::Value> = match &action {
            Some(act) => {
                // Create operation
                let index_name = match &act.get("_index") {
                    Some(val) => match val.as_str() {
                        Some(val) => val,
                        None => "default",
                    },
                    None => "default",
                };
                let index_name = index_name.to_string();
                let doc_id = match &act.get("_id") {
                    Some(val) => match val.as_str() {
                        Some(val) => val.to_string(),
                        None => rng.u64(0..u64::MAX).to_string(),
                    },
                    None => rng.u64(0..u64::MAX).to_string(),
                };

                //POST new data
                let mut remote_addr = String::from("");
                let remote_ip = match req.remote() {
                    Some(remote) => match usiem::utilities::ip_utils::ipv4_from_str(remote) {
                        Ok(ip) => SiemIp::V4(ip),
                        Err(_) => {
                            remote_addr = remote.to_string();
                            SiemIp::V4(0)
                        }
                    },
                    None => SiemIp::V4(0),
                };

                match serde_json::from_str(line) {
                    Ok(json_content) => {
                        let mut log =
                            SiemLog::new(line.to_string(), utc.timestamp_millis(), remote_ip);
                        log.add_field("host.address", SiemField::from_str(remote_addr));
                        log.add_field("elastic.index", SiemField::from_str(index_name.to_string()));
                        log.set_event(SiemEvent::Json(json_content));
                        let _ = state.sender.send(log);
                        logs_indexed += 1;
                        to_ret.push(json!({
                            "create" : {
                                //"winlogbeat-7.8.1-2020.10.23-000002"
                                "_index" : &index_name,
                                "_type" : "_doc",
                                "_id" : doc_id,
                                "_version" : 1,
                                "result" : "created",
                                "_shards" : {
                                "total" : 2,
                                "successful" : 1,
                                "failed" : 0
                                },
                                "_seq_no" : 56926,
                                "_primary_term" : 1,
                                "status" : 201
                            }
                        }));
                    }
                    Err(_) => {
                        logs_failed += 1;
                        to_ret.push(json!({
                            "create" : {
                                //"winlogbeat-7.8.1-2020.10.23-000002"
                                "_index" : &index_name,
                                "_type" : "_doc",
                                "_id" : doc_id,
                                "_version" : 1,
                                "status" : 400,
                                "error" : {
                                    "type" : "request_error_engine_exception",
                                    "reason" : "Malformed JSON",
                                    "index_uuid" : "A0esEFoYQ06rco-CKyiklQ",
                                    "shard" : "0",
                                    "index" : &index_name
                                  }
                            }
                        }));
                    }
                }

                None
            }
            None => {
                if line.trim() == "" {
                    continue;
                }
                let act: serde_json::Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => {
                        return invalid_parameter(format!(
                            "Not a valid \"operation\" in line {}",
                            n_line
                        ))
                    }
                };
                match act.get("create") {
                    Some(_) => Some(act),
                    None => {
                        // TODO: Support more operations
                        None
                    }
                }
            }
        };
        action = act;
        n_line += 1;
    }
    let body = tide::Body::from_json(&json!({
        "took" : logs_indexed,
        "errors" : logs_failed > 0,
        "items" : to_ret
    }));
    match body {
        Ok(body) => tide::Result::Ok(tide::Response::from(body)),
        Err(e) => tide::Result::Err(e),
    }
}

async fn es_metadata(mut _req: Request<State>) -> tide::Result {
    let utc: DateTime<Utc> = Utc::now();
    let utc_s = format!("{:?}", utc);
    let body = tide::Body::from_json(&json!({
        "name": "usiem-elastic-input",
        "cluster_name": "uSIEM",
        "cluster_uuid": "Qpv8TMuHSS2qmiLdNKFxsg",
        "version": {
            "number": "7.8.0",
            "build_flavor": "default",
            "build_type": "deb",
            "build_hash": "757314695644ea9a1dc2fecd26d1a43856725e65",
            "build_date": utc_s,
            "build_snapshot": false,
            "lucene_version": "8.5.1",
            "minimum_wire_compatibility_version": "6.8.0",
            "minimum_index_compatibility_version": "6.0.0-beta1"
        },
        "tagline": "You Know, for Search"
    }));
    match body {
        Ok(body) => tide::Result::Ok(tide::Response::from(body)),
        Err(e) => tide::Result::Err(e),
    }
}

async fn es_license(mut _req: Request<State>) -> tide::Result {
    let utc: DateTime<Utc> = Utc::now();
    let utc_s = format!("{:?}", utc);
    let body = tide::Body::from_json(&json!({
      "license" : {
        "status" : "active",
        "uid" : "f008ea32-5444-4bae-ae49-f05e36af8897",
        "type" : "basic",
        "issue_date" : utc_s,
        "issue_date_in_millis" : 159335844,
        "max_nodes" : 1000,
        "issued_to" : "uSIEM",
        "issuer" : "elasticsearch",
        "start_date_in_millis" : -1
      }
    }));
    match body {
        Ok(body) => tide::Result::Ok(tide::Response::from(body)),
        Err(e) => tide::Result::Err(e),
    }
}

async fn es_xpack(mut _req: Request<State>) -> tide::Result {
    let utc: DateTime<Utc> = Utc::now();
    let utc_s = format!("{:?}", utc);
    let body = tide::Body::from_json(
        &json!({"build":{"hash":"757314695644ea9a1dc2fecd26d1a43856725e65","date":utc_s},"license":{"uid":"f008ea32-5444-4bae-ae49-f05e36af8897","type":"basic","mode":"basic","status":"active"},"features":{"analytics":{"available":true,"enabled":true},"ccr":{"available":false,"enabled":true},"enrich":{"available":true,"enabled":true},"eql":{"available":true,"enabled":false},"flattened":{"available":true,"enabled":true},"frozen_indices":{"available":true,"enabled":true},"graph":{"available":false,"enabled":true},"ilm":{"available":true,"enabled":true},"logstash":{"available":false,"enabled":true},"ml":{"available":false,"enabled":false,"native_code_info":{"version":"N/A","build_hash":"N/A"}},"monitoring":{"available":true,"enabled":true},"rollup":{"available":true,"enabled":true},"security":{"available":true,"enabled":false},"slm":{"available":true,"enabled":true},"spatial":{"available":true,"enabled":true},"sql":{"available":true,"enabled":true},"transform":{"available":true,"enabled":true},"vectors":{"available":true,"enabled":true},"voting_only":{"available":true,"enabled":true},"watcher":{"available":false,"enabled":true}},"tagline":"You know, for X"}),
    );
    match body {
        Ok(body) => tide::Result::Ok(tide::Response::from(body)),
        Err(e) => tide::Result::Err(e),
    }
}

async fn es_ilm_policy(req: Request<State>) -> tide::Result {
    let utc: DateTime<Utc> = Utc::now();
    let utc_s = format!("{:?}", utc);
    let policy = req.param("policy")?;
    let body = tide::Body::from_json(
        &json!({policy:{"version":1,"modified_date":utc_s,"policy":{"phases":{"hot":{"min_age":"0ms","actions":{"rollover":{"max_size":"50gb","max_age":"30d"}}}}}}}),
    );
    match body {
        Ok(body) => tide::Result::Ok(tide::Response::from(body)),
        Err(e) => tide::Result::Err(e),
    }
}

async fn es_cat_templates(req: Request<State>) -> tide::Result {
    let v = req.param("temp_id")?;
    let body = tide::Body::from_string(format!("{} [{}-*] 1  ", v, v));
    tide::Result::Ok(tide::Response::from(body))
}
