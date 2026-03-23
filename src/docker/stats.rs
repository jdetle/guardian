use crate::state::DockerState;
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Deserialize)]
struct Container {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Names")]
    names: Vec<String>,
    #[serde(rename = "State")]
    state: String,
}

#[derive(Deserialize)]
struct ContainerStats {
    cpu_stats: Option<CpuStats>,
    precpu_stats: Option<CpuStats>,
    memory_stats: Option<MemoryStats>,
}

#[derive(Deserialize)]
struct CpuStats {
    cpu_usage: Option<CpuUsage>,
    system_cpu_usage: Option<u64>,
    online_cpus: Option<u32>,
}

#[derive(Deserialize)]
struct CpuUsage {
    total_usage: Option<u64>,
}

#[derive(Deserialize)]
struct MemoryStats {
    usage: Option<u64>,
}

pub struct DockerClient {
    socket_path: String,
}

impl DockerClient {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
        }
    }

    pub fn available(&self) -> bool {
        std::path::Path::new(&self.socket_path).exists()
    }

    pub fn aggregate_stats(&self) -> DockerState {
        let containers = match self.list_running_containers() {
            Ok(c) => c,
            Err(_) => return DockerState::default(),
        };

        let count = containers.len() as u32;
        let mut total_cpu = 0.0f64;
        let mut total_mem = 0u64;

        for container in &containers {
            if let Ok(stats) = self.get_container_stats(&container.id) {
                if let (Some(cpu), Some(precpu)) = (&stats.cpu_stats, &stats.precpu_stats) {
                    let cpu_delta = cpu
                        .cpu_usage
                        .as_ref()
                        .and_then(|u| u.total_usage)
                        .unwrap_or(0)
                        .saturating_sub(
                            precpu
                                .cpu_usage
                                .as_ref()
                                .and_then(|u| u.total_usage)
                                .unwrap_or(0),
                        );
                    let sys_delta = cpu
                        .system_cpu_usage
                        .unwrap_or(0)
                        .saturating_sub(precpu.system_cpu_usage.unwrap_or(0));
                    let ncpus = cpu.online_cpus.unwrap_or(1);

                    if sys_delta > 0 {
                        total_cpu +=
                            (cpu_delta as f64 / sys_delta as f64) * ncpus as f64 * 100.0;
                    }
                }

                if let Some(mem) = &stats.memory_stats {
                    total_mem += mem.usage.unwrap_or(0);
                }
            }
        }

        DockerState {
            running_containers: count,
            total_cpu_percent: (total_cpu * 10.0).round() / 10.0,
            total_memory_mb: total_mem / (1024 * 1024),
        }
    }

    fn list_running_containers(&self) -> Result<Vec<Container>, String> {
        let response = self
            .http_get("/containers/json")
            .map_err(|e| format!("list containers: {e}"))?;
        serde_json::from_str(&response).map_err(|e| format!("parse containers: {e}"))
    }

    fn get_container_stats(&self, id: &str) -> Result<ContainerStats, String> {
        let path = format!("/containers/{id}/stats?stream=false&one-shot=true");
        let response = self
            .http_get(&path)
            .map_err(|e| format!("stats {id}: {e}"))?;
        serde_json::from_str(&response).map_err(|e| format!("parse stats: {e}"))
    }

    fn http_get(&self, path: &str) -> std::io::Result<String> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;

        let request = format!(
            "GET {path} HTTP/1.0\r\nHost: localhost\r\nAccept: application/json\r\n\r\n"
        );
        stream.write_all(request.as_bytes())?;
        stream.flush()?;

        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        let body_start = response
            .find("\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(0);
        Ok(response[body_start..].to_string())
    }

    pub fn container_names(&self) -> Vec<String> {
        self.list_running_containers()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|c| c.names)
            .map(|n| n.trim_start_matches('/').to_string())
            .collect()
    }
}
