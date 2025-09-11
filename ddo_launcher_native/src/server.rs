#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Server {
    pub name: String,
    pub lobby_ip: String,
    pub lobby_port: u16,
    pub dl_ip: String,
    pub dl_port: u16,
}
