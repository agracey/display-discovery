use akri_discovery_utils::discovery::{
    discovery_handler::{ DISCOVERED_DEVICES_CHANNEL_CAPACITY},
    v0::{discovery_handler_server::DiscoveryHandler, Device, DiscoverRequest, DiscoverResponse, Mount},
    DiscoverStream,
};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tonic::{Response, Status};

use std::collections::HashSet;
use std::time::Duration;

use log::{trace, info, error};

mod wrappers;
use wrappers::{
    get_devnode, get_devpath, Enumerator, create_enumerator
};


const DISCOVERY_INTERVAL_SECS: u64 = 10;


pub struct DiscoveryHandlerImpl {
    register_sender: tokio::sync::mpsc::Sender<()>,
}

impl DiscoveryHandlerImpl {
    pub fn new(register_sender: tokio::sync::mpsc::Sender<()>) -> Self {
        DiscoveryHandlerImpl { register_sender }
    }
}

#[async_trait]
impl DiscoveryHandler for DiscoveryHandlerImpl {
    type DiscoverStream = DiscoverStream;
    async fn discover(
        &self,
        _request: tonic::Request<DiscoverRequest>,
    ) -> Result<Response<Self::DiscoverStream>, Status> {
        
        info!("discover - called for display protocol");
        let register_sender = self.register_sender.clone();
        let (discovered_devices_sender, discovered_devices_receiver) =
            mpsc::channel(DISCOVERED_DEVICES_CHANNEL_CAPACITY);


        let mut previously_discovered_devices: Vec<Device> = Vec::new();

        tokio::spawn(async move {
            loop {
                trace!("discover - displays");
                // Before each iteration, check if receiver has dropped
                if discovered_devices_sender.is_closed() {
                    error!("discover - channel closed ... attempting to re-register with Agent");
                    register_sender.send(()).await.unwrap();
                    break;
                }
                
                let mut devpaths: HashSet<String> = HashSet::new();
                {
                    let enumerator = create_enumerator();

                    let paths = find_devices(enumerator).unwrap();
                    paths.into_iter().for_each(|path| {
                        devpaths.insert(path);
                    });
                }
                trace!(
                    "discover - mapping and returning devices at devpaths {:?}",
                    devpaths
                );
                let discovered_devices = devpaths
                    .into_iter()
                    .map(|path| {
                        let mut properties = std::collections::HashMap::new();
                        properties.insert("DISPLAY_DEV".to_string(), path.clone());
                        let mount = Mount {
                            container_path: path.clone(),
                            host_path: path.clone(),
                            read_only: true,
                        };
                        // TODO: use device spec
                        Device {
                            id: path,
                            properties,
                            mounts: vec![mount],
                            device_specs: Vec::default(),
                        } 
                    })
                    .collect::<Vec<Device>>();
                let mut changed_device_list = false;
                let mut matching_device_count = 0;
                discovered_devices.iter().for_each(|device| {
                    if !previously_discovered_devices.contains(device) {
                        changed_device_list = true;
                    } else {
                        matching_device_count += 1;
                    }
                });
                if changed_device_list
                    || matching_device_count != previously_discovered_devices.len()
                {
                    info!("discover - sending updated device list");
                    previously_discovered_devices = discovered_devices.clone();
                    if let Err(e) = discovered_devices_sender
                        .send(Ok(DiscoverResponse {
                            devices: discovered_devices,
                        }))
                        .await
                    {
                        error!(
                            "discover - for display failed to send discovery response with error {}",
                            e
                        );
                        register_sender.send(()).await.unwrap();
                        break;
                    }
                }
                sleep(Duration::from_secs(DISCOVERY_INTERVAL_SECS)).await;
            }
        });
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            discovered_devices_receiver,
        )))
    }
}


/// This searches for devices that match the UdevFilters and returns their devpaths
fn find_devices(
    enumerator: impl Enumerator,
) -> std::io::Result<Vec<String>> {
    let mut enumerator = enumerator;
    trace!("find_devices - connected displays");

    enumerator.match_subsystem("drm").unwrap();
    enumerator.match_attribute("status","connected").unwrap();

    let final_devices: Vec<udev::Device> = enumerator.scan_devices()?.collect();

    let device_devpaths: Vec<String> = final_devices
        .into_iter()
        .filter_map(|device| {
            if let Some(devnode) = get_devnode(&device) {
                Some(devnode.to_str().unwrap().to_string())
            } else {
                Some(get_devpath(&device).to_str().unwrap().to_string())
            }
        })
        .collect();

    Ok(device_devpaths)
}
