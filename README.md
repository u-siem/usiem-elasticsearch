# usiem-elasticsearch
Input and Ouput logging components for uSIEM

### ElasticSearch Output
First of all, this is a simple component that needs a lot of improvement.
```rust
// The configuration must be inside the code to reduce human errors in configuration files
let config = ElasticOuputConfig {
    commit_max_messages : 10, // Max number of elements in a request to elasticsearch
    commit_timeout : 1,// Timeot for the connection
    commit_time : 1,// Max milliseconds between requests to elasticsearch
    cache_size : 20, //Cache to store log retries
    elastic_address : String::from("http://127.0.0.1:9200"),
    elastic_stream : String::from("log-integration-test"),
    bearer_token : None //Some(String::new("API_TOKEN"))
};
let mut es_output = ElasticSearchOutput::new(config);
// Configure component: register data schema...
//...

// Register the component in the kernel, it will be responsible for autoscaling and starting copies of the component 
kernel.add_output(es_output);
// The kernel is a work in progress so it's not available
```