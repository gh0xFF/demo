use std::collections::HashMap;
use std::error::Error;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, mpsc};
use sqlx::migrate::MigrateDatabase;
use sqlx::{Sqlite, SqlitePool};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:8080").unwrap();
    println!("Listening for connections on port {}", 8080);

    let db_pool = Arc::new(new_conn("sqlite://sqlite1.db").await.unwrap());  

    let (tx, rx) = mpsc::channel();
    let rx = Arc::new(Mutex::new(rx));

    for _ in 0..16 {
        let rx = Arc::clone(&rx);
        let db_pool = Arc::clone(&db_pool);
        tokio::spawn(async move {
            worker(rx, db_pool).await;
        });
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    tx.send(stream).unwrap();
                });
            },
            Err(e) => println!("Unable to connect: {}", e),
        }
    }

}

#[inline]
async fn handle_connection(mut stream: TcpStream, pool: Arc<sqlx::Pool<Sqlite>>) {
    let mut buf = [0u8; 4096];
    match stream.read(&mut buf) {
        Ok(_) => {
            let req_str = String::from_utf8_lossy(&mut buf);
            let parts: Vec<&str> = req_str.split_whitespace().collect();
    
            if parts.len() < 1 {
                println!("something went wrong [{:?}]", req_str);
                return ;
            }

            let pt = parts[1];
            if pt.len() == 0 {
                println!("something went wrong [{:?}]", pt);
                return ;
            }
            let path: &str = &pt[..parts[1].find('?').unwrap_or(pt.len())];

            match path {
                "/" => send_response(stream, b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>Hola</body></html>\r\n"),
                "/get" => { // GET / 127.0.0.1:8080/get?name=hehe&id=3 HTTP/1.1
                    let req_params = req_str.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("");
                    if let Some(uid) = parse_query_string(req_params).get("id") {
                        match sqlx::query_as::<_, User>("SELECT * FROM users where id = $1").bind(uid).fetch_one(pool.as_ref()).await {
                            Ok(u) => {
                                let rsp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/json; charset=UTF-8\r\n\r\nname - {}\nid - {}\r\n", u.name, u.id);
                                send_response(stream, rsp.as_bytes());
                            },
                            Err(e) => {
                                send_response(stream, b"HTTP/1.1 400 BAD REQUEST\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>400 Bad Request</body></html>\r\n");
                                println!("/get error: {}", e);
                            },
                        }
                    } else {
                        send_response(stream, b"HTTP/1.1 400 BAD REQUEST\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>400 Bad Request</body></html>\r\n");
                    }
                },
                "/health" => {
                    match sqlx::query("SELECT 1").fetch_one(pool.as_ref()).await {
                        Ok(_) => send_response(stream, b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nHealthy\r\n"),
                        Err(e) => {
                            send_response(stream, b"HTTP/1.1 500 INTERNAL SERVER ERROR\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nUnHealthy\r\n");
                            println!("healthcheck error: {}", e)
                        },
                    }
                }
                _ => {
                    send_response(stream, b"HTTP/1.1 400 BAD REQUEST\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n<html><body>400 Bad Request</body></html>\r\n");
                    println!("default branch");
                }
            }
        }
        Err(e) => println!("Unable to read stream: {}", e),
    }
}

#[inline]
fn send_response(mut stream: TcpStream, resp: &[u8]) {
    match stream.write_all(resp) {
        Err(e) => println!("Failed sending response: {}", e),
        Ok(_) => (),
    }
    let _ = stream.flush();
}

async fn new_conn(path: &str) -> Result<sqlx::Pool<Sqlite>, Box<dyn Error>> {
    let mut is_empty = false;
    if !Sqlite::database_exists(path).await.unwrap_or(false) {
        Sqlite::create_database(path).await?;
        is_empty = true
    }

    let pool = SqlitePool::connect(path).await.unwrap();
    sqlx::query(r#"CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY NOT NULL, name VARCHAR(250) NOT NULL); 
    CREATE INDEX IF NOT EXISTS users_id_idx ON users (id)"#).execute(&pool).await?;

    if is_empty {
        for i in 0..10000 {
            let mut name = String::from("value");
            name.push_str(i.to_string().as_str());
            sqlx::query("INSERT INTO users (name) VALUES (?)").bind(name).execute(&pool).await.unwrap();
        }
    }
    Ok(pool)
}

#[inline]
fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query_str) = query.split('?').nth(1) {
        for param in query_str.split('&') {
            let mut key_value = param.split('=');
            if let (Some(key), Some(value)) = (key_value.next(), key_value.next()) {
                params.insert(key.to_string(), value.to_string());
            }
        }
    }
    return params
}

#[derive(sqlx::FromRow)]
struct User {
    id: u32,
    name: String,
}

async fn worker(rx: Arc<Mutex<mpsc::Receiver<TcpStream>>>, db_pool: Arc<sqlx::Pool<Sqlite>>) {
    while let Ok(stream) = rx.lock().await.recv() {
        handle_connection(stream, db_pool.clone()).await;
    }
}
