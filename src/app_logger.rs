use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, SystemTime};
use chrono::Local;
use log::{Level, LevelFilter, Metadata, Record};

/// Capacidad del canal sin bloqueo para logs en memoria
const LOG_QUEUE_CAPACITY: usize = 50_000;
/// Tamaño máximo por archivo de log antes de rotar (25 MB)
const MAX_LOG_FILE_BYTES: u64 = 25 * 1024 * 1024;
/// Número máximo de archivos históricos rotados
const MAX_BACKUP_LOGS: usize = 5;

#[derive(Debug, Clone)]
pub enum LogCategory {
    UiAction,
    UiInput,
    UiError,
    DbOp,
    DbRead,
    DbError,
    General(Level),
}

#[derive(Debug, Clone)]
pub struct AppLogMessage {
    pub timestamp: String,
    pub category: LogCategory,
    pub target: String,
    pub message: String,
}

struct AppLoggerChannel {
    sender: SyncSender<AppLogMessage>,
    dropped_count: std::sync::atomic::AtomicUsize,
}

static LOGGER_CHANNEL: OnceLock<AppLoggerChannel> = OnceLock::new();
static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Inicializa el sistema de logs global asíncrono y no bloqueante.
/// Crea el hilo de fondo y el archivo de log en `./logs/docu_ffacd.log`.
pub fn init_app_logger() {
    if IS_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }

    let (sender, receiver) = sync_channel::<AppLogMessage>(LOG_QUEUE_CAPACITY);

    LOGGER_CHANNEL.set(AppLoggerChannel {
        sender,
        dropped_count: std::sync::atomic::AtomicUsize::new(0),
    }).ok();

    // Iniciar hilo trabajador en segundo plano con baja prioridad de E/S
    let _ = thread::Builder::new()
        .name("async-app-logger".to_string())
        .spawn(move || {
            run_logger_worker(receiver);
        });

    // Registrar como logger para la crate `log`
    struct GlobalLogBridge;
    impl log::Log for GlobalLogBridge {
        fn enabled(&self, _metadata: &Metadata) -> bool {
            true
        }

        fn log(&self, record: &Record) {
            let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
            let cat = match record.level() {
                Level::Error => LogCategory::General(Level::Error),
                Level::Warn => LogCategory::General(Level::Warn),
                Level::Info => LogCategory::General(Level::Info),
                Level::Debug => LogCategory::General(Level::Debug),
                Level::Trace => LogCategory::General(Level::Trace),
            };

            enqueue_log(AppLogMessage {
                timestamp: now,
                category: cat,
                target: record.target().to_string(),
                message: format!("{}", record.args()),
            });
        }

        fn flush(&self) {}
    }

    let _ = log::set_boxed_logger(Box::new(GlobalLogBridge));
    log::set_max_level(LevelFilter::Info);

    log_general_info("Sistema de logging no bloqueante inicializado correctamente");
}

fn get_log_dir() -> PathBuf {
    // 1. Intentar en la subcarpeta ./logs del directorio de trabajo actual (ideal para modo portable o desarrollo)
    if let Ok(current) = std::env::current_dir() {
        let local_logs = current.join("logs");
        if fs::create_dir_all(&local_logs).is_ok() {
            let test_file = local_logs.join(".write_test");
            if File::create(&test_file).and_then(|_| fs::remove_file(&test_file)).is_ok() {
                return local_logs;
            }
        }
    }

    // 2. Si no se puede escribir en la carpeta actual (ej. instalado en Program Files o /usr/bin), usar ruta de usuario
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(appdata).join("docu_ffacd").join("logs");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join("Library").join("Logs").join("docu_ffacd");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let p = PathBuf::from(home).join(".local").join("share").join("docu_ffacd").join("logs");
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }

    PathBuf::from("logs")
}

fn run_logger_worker(receiver: Receiver<AppLogMessage>) {
    let log_dir = get_log_dir();
    let current_log_path = log_dir.join("docu_ffacd.log");

    let mut current_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&current_log_path)
        .ok();

    let mut buffer = String::with_capacity(32 * 1024);
    let mut last_flush = SystemTime::now();

    loop {
        // Espera no bloqueante con timeout para vaciar periódicamente a disco
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(msg) => {
                let category_str = match msg.category {
                    LogCategory::UiAction => "[UI_ACTION]",
                    LogCategory::UiInput => "[UI_INPUT]",
                    LogCategory::UiError => "[UI_ERROR]",
                    LogCategory::DbOp => "[DB_OPERACION]",
                    LogCategory::DbRead => "[DB_LECTURA]",
                    LogCategory::DbError => "[DB_ERROR]",
                    LogCategory::General(Level::Error) => "[ERROR]",
                    LogCategory::General(Level::Warn) => "[WARN]",
                    LogCategory::General(Level::Info) => "[INFO]",
                    LogCategory::General(Level::Debug) => "[DEBUG]",
                    LogCategory::General(Level::Trace) => "[TRACE]",
                };

                let line = format!("{} {:<14} [{}] {}\n", msg.timestamp, category_str, msg.target, msg.message);
                buffer.push_str(&line);

                // Imprimir a consola solo errores o advertencias para no saturar el flujo de la terminal
                match msg.category {
                    LogCategory::UiError | LogCategory::DbError | LogCategory::General(Level::Error) | LogCategory::General(Level::Warn) => {
                        eprint!("{}", line);
                    }
                    _ => {}
                }

                // Escribir a disco si el buffer alcanza ~16KB o periódicamente
                if buffer.len() >= 16 * 1024 || last_flush.elapsed().map(|d| d.as_millis() >= 300).unwrap_or(true) {
                    write_buffer_to_file(&mut buffer, &mut current_file, &current_log_path, &log_dir);
                    last_flush = SystemTime::now();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if !buffer.is_empty() {
                    write_buffer_to_file(&mut buffer, &mut current_file, &current_log_path, &log_dir);
                    last_flush = SystemTime::now();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if !buffer.is_empty() {
                    write_buffer_to_file(&mut buffer, &mut current_file, &current_log_path, &log_dir);
                }
                break;
            }
        }
    }
}

fn write_buffer_to_file(
    buffer: &mut String,
    current_file: &mut Option<File>,
    current_log_path: &PathBuf,
    log_dir: &PathBuf,
) {
    if buffer.is_empty() {
        return;
    }

    // Verificar rotación
    if let Ok(meta) = fs::metadata(current_log_path) {
        if meta.len() >= MAX_LOG_FILE_BYTES {
            *current_file = None; // cerrar descriptor
            rotate_logs(log_dir);
            *current_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(current_log_path)
                .ok();
        }
    }

    if current_file.is_none() {
        *current_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(current_log_path)
            .ok();
    }

    if let Some(file) = current_file.as_mut() {
        let _ = file.write_all(buffer.as_bytes());
        let _ = file.flush();
    }

    buffer.clear();
}

fn rotate_logs(log_dir: &PathBuf) {
    let main_log = log_dir.join("docu_ffacd.log");
    for i in (1..MAX_BACKUP_LOGS).rev() {
        let src = log_dir.join(format!("docu_ffacd_{}.log", i));
        let dst = log_dir.join(format!("docu_ffacd_{}.log", i + 1));
        if src.exists() {
            let _ = fs::rename(&src, &dst);
        }
    }
    let first_backup = log_dir.join("docu_ffacd_1.log");
    let _ = fs::rename(&main_log, &first_backup);
}

fn enqueue_log(msg: AppLogMessage) {
    if let Some(channel) = LOGGER_CHANNEL.get() {
        match channel.sender.try_send(msg) {
            Ok(_) => {}
            Err(TrySendError::Full(_)) => {
                // Para no bloquear la UI ni generar cuellos de botella, registramos conteo de drops
                channel.dropped_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

// =========================================================================
// FUNCIONES PÚBLICAS DE LOGGING ESTRUCTURADO Y DE ALTO RENDIMIENTO
// =========================================================================

/// Registra una acción de usuario en la interfaz visual (clicks en botones, selección de filas, menús)
pub fn log_ui_action(view: &str, section: &str, element: &str, action: &str, details: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let msg = format!(
        "[VISTA: {}] [SECCIÓN: {}] [ELEMENTO: {}] ACCIÓN: {} | DETALLES: {}",
        view, section, element, action, details
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::UiAction,
        target: format!("UI::{}", view),
        message: msg,
    });
}

/// Registra una acción o evento de usuario en la interfaz visual (formato simplificado: vista, sección, acción, detalles)
pub fn log_ui_event(view: &str, section: &str, action: &str, details: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let msg = format!(
        "[VISTA: {}] [SECCIÓN: {}] ACCIÓN: {} | DETALLES: {}",
        view, section, action, details
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::UiAction,
        target: format!("UI::{}", view),
        message: msg,
    });
}


/// Registra cambios en inputs de usuario (textboxes, selects, comboboxes, switches, calendarios)
pub fn log_ui_input(view: &str, section: &str, element: &str, field_name: &str, new_val: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let msg = format!(
        "[VISTA: {}] [SECCIÓN: {}] [CONTROL: {}] CAMPO: '{}' -> NUEVO VALOR: '{}'",
        view, section, element, field_name, new_val
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::UiInput,
        target: format!("UI::{}", view),
        message: msg,
    });
}

/// Registra un error ocurrido en una vista o interacción de la interfaz
pub fn log_ui_error(view: &str, section: &str, element: &str, error: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let msg = format!(
        "[VISTA: {}] [SECCIÓN: {}] [ELEMENTO: {}] ERROR: {}",
        view, section, element, error
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::UiError,
        target: format!("UI::{}", view),
        message: msg,
    });
}

/// Registra una operación de modificación en la base de datos (INSERT, UPDATE, DELETE)
pub fn log_db_op(
    op_name: &str,
    tables: &[&str],
    query_sql: &str,
    data_summary: &str,
    rows_affected: Option<usize>,
) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let tables_str = tables.join(", ");
    let rows_str = rows_affected.map(|r| r.to_string()).unwrap_or_else(|| "N/A".to_string());
    let clean_query = query_sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let msg = format!(
        "OP: {} | TABLAS: [{}] | FILAS AFECTADAS: {} | DATOS GUARDADOS: {} | QUERY: {}",
        op_name, tables_str, rows_str, data_summary, clean_query
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::DbOp,
        target: "DB::WRITE".to_string(),
        message: msg,
    });
}

/// Registra una operación de lectura en la base de datos (SELECT)
pub fn log_db_read(
    tables: &[&str],
    query_sql: &str,
    filters_params: &str,
    rows_read: usize,
) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let tables_str = tables.join(", ");
    let clean_query = query_sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let msg = format!(
        "SELECT | TABLAS: [{}] | FILAS OBTENIDAS: {} | FILTROS: {} | QUERY: {}",
        tables_str, rows_read, filters_params, clean_query
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::DbRead,
        target: "DB::READ".to_string(),
        message: msg,
    });
}

/// Registra un error en una consulta o transacción a la base de datos
pub fn log_db_error(
    op_name: &str,
    tables: &[&str],
    query_sql: &str,
    error: &str,
) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    let tables_str = tables.join(", ");
    let clean_query = query_sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let msg = format!(
        "FALLO BD ({}) | TABLAS: [{}] | ERROR: {} | QUERY: {}",
        op_name, tables_str, error, clean_query
    );
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::DbError,
        target: "DB::ERROR".to_string(),
        message: msg,
    });
}

/// Registra una operación de modificación en una sola tabla (helper ergonómico: tabla, operación, datos, query)
pub fn log_db_write(table: &str, op_name: &str, data_summary: &str, query_sql: &str) {
    log_db_op(op_name, &[table], query_sql, data_summary, None);
}

/// Registra un fallo en base de datos para una tabla (helper ergonómico: tabla, operación, error)
pub fn log_db_simple_error(table: &str, op_name: &str, error: &str) {
    log_db_error(op_name, &[table], "", error);
}


pub fn log_system_info(tag: &str, msg: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::General(Level::Info),
        target: format!("SISTEMA::{}", tag),
        message: msg.to_string(),
    });
}

pub fn log_general_info(msg: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string();
    enqueue_log(AppLogMessage {
        timestamp: now,
        category: LogCategory::General(Level::Info),
        target: "APP::CORE".to_string(),
        message: msg.to_string(),
    });
}
