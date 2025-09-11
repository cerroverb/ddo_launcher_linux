use std::time::Duration;

use directories::ProjectDirs;
use eframe::egui;
use tokio::{
    process::Command,
    sync::{mpsc, oneshot},
};

use crate::server::Server;

mod server;

const CONFIG_FILE_NAME: &str = "config.json";

enum TokioRequest {
    BeginRequestRepaintLoop(egui::Context),
    Login(LoginRequest),
    Register(RegisterRequest),
    Shutdown,
}

enum TokioResponse {
    Status(String),
    ChildProcessActive(bool),
}

struct LoginRequest {
    account: Account,
    exe_path: String,
    prefix_path: String,
}

struct RegisterRequest {
    account: Account,
}

struct Account {
    name: String,
    password: String,
    server: Server,
}

#[derive(Clone, Copy)]
enum DLServerAction {
    Login,
    Register,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
struct DLServerRequestData<'a> {
    action: &'a str,
    account: &'a str,
    password: &'a str,
    email: &'a str,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DLServerResponse {
    error: Option<String>,
    message: Option<String>,
    token: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let Some(dirs) = ProjectDirs::from("", "cndofx", "ddo_launcher_native") else {
        eprintln!("Error: Unable to determine project storage directories");
        return Ok(());
    };

    let config_path = {
        let mut config = dirs.config_dir().to_path_buf();
        config.push(CONFIG_FILE_NAME);
        config
    };
    dbg!(config_path.display());

    let config = match config_path.try_exists() {
        Err(e) => {
            eprintln!("Error: Unable to check if config file exists: {e}");
            return Ok(());
        }
        Ok(true) if !config_path.is_file() => {
            eprintln!(
                "Error: Config file path exists but it is not a file ({})",
                config_path.display()
            );
            return Ok(());
        }
        Ok(true) => {
            let config_str = std::fs::read_to_string(&config_path).unwrap();
            let config: AppConfig = serde_json::from_str(&config_str).unwrap();
            println!("loaded config file");
            config
        }
        Ok(false) => {
            println!("creating new default config");
            AppConfig::default()
        }
    };

    let args: Vec<String> = std::env::args().collect();
    if args.len() > 2 {
        eprintln!("usage: ddo_launcher_native [server name]");
        return Ok(());
    } else if args.len() == 2 {
        run_simple(&args[1], config)?;
    } else {
        run_gui(config, dirs)?;
    }

    Ok(())
}

fn run_simple(server_name: &str, config: AppConfig) -> anyhow::Result<()> {
    let Some(server) = config.servers.iter().find(|s| &s.name == server_name) else {
        anyhow::bail!(
            "error: No server named \"{}\" found in config file",
            server_name
        )
    };

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on((async || -> anyhow::Result<()> {
            let account = Account {
                name: config.account.clone(),
                password: config.password.clone(),
                server: server.clone(),
            };

            let response = dl_server_action(DLServerAction::Login, &account).await?;

            if let Some(error) = &response.error {
                anyhow::bail!("DL Server error: {error}");
            }

            if let Some(token) = &response.token {
                spawn_umu(
                    LoginRequest {
                        account: account,
                        exe_path: config.exe_path.clone(),
                        prefix_path: config.prefix_path.clone(),
                    },
                    token.clone(),
                )
                .await;
            } else {
                anyhow::bail!(
                    "DL Server response did not include a token, message: \"{}\"",
                    response.message.unwrap_or_default(),
                );
            }

            Ok(())
        })())
}

fn run_gui(config: AppConfig, dirs: ProjectDirs) -> anyhow::Result<()> {
    let (main_tx, tokio_rx) = mpsc::channel::<TokioRequest>(8);
    let (tokio_tx, main_rx) = mpsc::channel::<TokioResponse>(8);

    let tokio_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(tokio_message_loop(tokio_tx, tokio_rx));
        println!("tokio thread exited");
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([320.0, 300.0]),
        ..Default::default()
    };
    let eframe_result = eframe::run_native(
        "DDO Launcher",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(config, main_tx.clone(), main_rx, dirs)))),
    );

    main_tx.blocking_send(TokioRequest::Shutdown).unwrap();
    tokio_thread.join().unwrap();

    match eframe_result {
        Ok(_) => {}
        Err(e) => anyhow::bail!("{e}"),
    }

    Ok(())
}

async fn tokio_message_loop(tx: mpsc::Sender<TokioResponse>, mut rx: mpsc::Receiver<TokioRequest>) {
    let mut cancel_repaint_tx: Option<oneshot::Sender<()>> = None;
    let mut repaint_task_handle: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(request) = rx.recv().await {
        println!("tokio thread got request");
        match request {
            TokioRequest::BeginRequestRepaintLoop(ctx) => {
                if let Some(old_tx) = cancel_repaint_tx.take() {
                    old_tx.send(()).unwrap();
                }
                let (new_cancel_repaint_tx, mut cancel_repaint_rx) = oneshot::channel::<()>();
                cancel_repaint_tx = Some(new_cancel_repaint_tx);

                let handle = tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                                ctx.request_repaint();
                            }
                            _ = &mut cancel_repaint_rx => {
                                println!("breaking request repaint loop");
                                break;
                            }
                        }
                    }
                    println!("repaint loop exited");
                });
                repaint_task_handle = Some(handle);
            }
            TokioRequest::Login(login) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    println!(
                        "tokio thread got login request (account {}, password {})",
                        login.account.name, login.account.password
                    );

                    let response = dl_server_action(DLServerAction::Login, &login.account)
                        .await
                        .unwrap();

                    if let Some(error) = &response.error {
                        tx.send(TokioResponse::Status(error.clone())).await.unwrap();
                    } else {
                        let message = response.message.as_ref().unwrap();
                        tx.send(TokioResponse::Status(message.clone()))
                            .await
                            .unwrap();
                        if message == "Login Success" {
                            tx.send(TokioResponse::ChildProcessActive(true))
                                .await
                                .unwrap();
                            spawn_umu(login, response.token.unwrap()).await;
                            tx.send(TokioResponse::ChildProcessActive(false))
                                .await
                                .unwrap();
                        }
                    }
                });
            }
            TokioRequest::Register(register) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    println!(
                        "tokio thread got register request (account {}, password {})",
                        register.account.name, register.account.password
                    );

                    let response = dl_server_action(DLServerAction::Register, &register.account)
                        .await
                        .unwrap();

                    if let Some(error) = &response.error {
                        tx.send(TokioResponse::Status(error.clone())).await.unwrap();
                    } else {
                        tx.send(TokioResponse::Status(
                            response.message.as_ref().unwrap().clone(),
                        ))
                        .await
                        .unwrap();
                    }
                });
            }
            TokioRequest::Shutdown => {
                println!("tokio thread received shutdown request");
                break;
            }
        }
    }
    println!("tokio thread exited message loop");
    if let Some(cancel_repaint_tx) = cancel_repaint_tx {
        println!("cancelling request repaint loop");
        cancel_repaint_tx.send(()).unwrap();
    }
    if let Some(repaint_task_handle) = repaint_task_handle {
        repaint_task_handle.await.unwrap();
    }
}

async fn dl_server_action(
    action: DLServerAction,
    account: &Account,
) -> anyhow::Result<DLServerResponse> {
    let url = format!(
        "http://{}:{}/api/account",
        account.server.dl_ip, account.server.dl_port
    );
    dbg!(&url);

    let request_data = DLServerRequestData {
        action: match action {
            DLServerAction::Login => "login",
            DLServerAction::Register => "create",
        },
        account: &account.name,
        password: &account.password,
        email: "",
    };
    dbg!(&request_data);

    let client = reqwest::Client::new();
    let response = client.post(&url).json(&request_data).send().await?;
    let response: DLServerResponse = response.json().await?;
    dbg!(&response);

    Ok(response)
}

async fn spawn_umu(login: LoginRequest, token: String) {
    let mut child = Command::new("umu-run")
        .env("WINEPREFIX", &login.prefix_path)
        .arg(&login.exe_path)
        .arg(format!("addr={}", login.account.server.lobby_ip))
        .arg(format!("port={}", login.account.server.lobby_port))
        .arg(format!("token={}", token))
        .arg(format!(
            "DL=http://{}:{}/win/",
            login.account.server.dl_ip, login.account.server.dl_port
        ))
        .arg("LVer=03.04.003.20181115.0")
        .arg("RVer=3040008")
        .spawn()
        .unwrap();

    child.wait().await.unwrap();
}

struct App {
    config: AppConfig,
    status_text: String,
    child_process_active: bool,
    repaint_loop_started: bool,
    tx: mpsc::Sender<TokioRequest>,
    rx: mpsc::Receiver<TokioResponse>,
    dirs: ProjectDirs,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct AppConfig {
    account: String,
    password: String,
    remember_me: bool,
    servers: Vec<Server>,
    selected_server: usize,

    exe_path: String,
    prefix_path: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            account: String::new(),
            password: String::new(),
            remember_me: false,
            servers: vec![Server {
                name: "DDOn".into(),
                lobby_ip: "ddon.org".into(),
                lobby_port: 52100,
                dl_ip: "ddon.org".into(),
                dl_port: 52099,
            }],
            selected_server: 0,
            exe_path: String::new(),
            prefix_path: String::new(),
        }
    }
}

impl App {
    fn new(
        config: AppConfig,
        tx: mpsc::Sender<TokioRequest>,
        rx: mpsc::Receiver<TokioResponse>,
        dirs: ProjectDirs,
    ) -> Self {
        Self {
            config: config,
            status_text: String::new(),
            child_process_active: false,
            repaint_loop_started: false,
            tx: tx,
            rx: rx,
            dirs: dirs,
        }
    }

    fn login(&mut self) {
        if !self.verify_account_details() {
            return;
        }

        self.tx
            .blocking_send(TokioRequest::Login(LoginRequest {
                account: Account {
                    name: self.config.account.clone(),
                    password: self.config.password.clone(),
                    server: self.config.servers[self.config.selected_server].clone(),
                },
                exe_path: self.config.exe_path.clone(),
                prefix_path: self.config.prefix_path.clone(),
            }))
            .unwrap();
    }

    fn register(&mut self) {
        if !self.verify_account_details() {
            return;
        }

        self.tx
            .blocking_send(TokioRequest::Register(RegisterRequest {
                account: Account {
                    name: self.config.account.clone(),
                    password: self.config.password.clone(),
                    server: self.config.servers[self.config.selected_server].clone(),
                },
            }))
            .unwrap();
    }

    fn verify_account_details(&mut self) -> bool {
        if self.config.account.is_empty() {
            self.status_text = "Error: \"Account\" must not be empty".into();
            return false;
        }

        if self.config.password.is_empty() {
            self.status_text = "Error: \"Password\" must not be empty".into();
            return false;
        }

        if self.config.selected_server >= self.config.servers.len() {
            self.status_text = "Error: No server selected".into();
            return false;
        }

        true
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.repaint_loop_started {
            self.tx
                .blocking_send(TokioRequest::BeginRequestRepaintLoop(ctx.clone()))
                .unwrap();
            self.repaint_loop_started = true;
        }

        while !self.rx.is_empty() {
            println!("main thread got response");
            match self.rx.blocking_recv().unwrap() {
                TokioResponse::Status(s) => self.status_text = s,
                TokioResponse::ChildProcessActive(v) => self.child_process_active = v,
            }
        }

        egui::TopBottomPanel::bottom("bottom_status_panel").show(ctx, |ui| {
            ui.label(&self.status_text);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(!self.child_process_active, |ui| {
                ui.label("Account");
                ui.text_edit_singleline(&mut self.config.account);
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut self.config.password).password(true));
                ui.horizontal(|ui| {
                    if ui.button("Log In").clicked() {
                        println!("log in!");
                        self.login();
                    }
                    if ui.button("Register").clicked() {
                        println!("register!");
                        self.register();
                    }
                    ui.checkbox(&mut self.config.remember_me, "Remember Me");
                });

                ui.add_space(20.0);

                ui.horizontal(|ui| {
                    ui.label("Server");
                    egui::ComboBox::from_id_salt("select_server_combobox")
                        .selected_text(
                            self.config
                                .servers
                                .get(self.config.selected_server)
                                .map_or("<none>", |s| &s.name),
                        )
                        .show_ui(ui, |ui| {
                            for (i, server) in self.config.servers.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.config.selected_server,
                                    i,
                                    &server.name,
                                );
                            }
                        });
                    if ui.button("Edit").clicked() {
                        println!("edit servers!");
                    }
                });

                ui.add_space(20.0);

                ui.label("DDO.exe Path");
                ui.text_edit_singleline(&mut self.config.exe_path);
                ui.label("Wine Prefix Path");
                ui.text_edit_singleline(&mut self.config.prefix_path);
            });
        });
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        let mut config = self.config.clone();
        if !config.remember_me {
            config.account = String::new();
            config.password = String::new();
        }

        let config_path = {
            let mut path = self.dirs.config_dir().to_path_buf();
            path.push(CONFIG_FILE_NAME);
            path
        };

        let config_str = serde_json::to_string_pretty(&config).unwrap();

        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        std::fs::write(&config_path, config_str).unwrap();
    }
}
