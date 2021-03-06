#![feature(proc_macro_hygiene, decl_macro)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use rocket::{post, routes, State};
use rocket_contrib::json::Json;

// TODO Move these to a separate crate
use dit::comm::{Entry, Index, Key, PartySignup};
use dit::utils::Operation;

// TODO Now we need to send the project name with every message
// TODO Start handling server errors

#[post("/start-operation", format = "json", data = "<request>")]
fn start_operation(
    db: State<RwLock<HashMap<String, Project>>>,
    request: Json<(String, Operation)>,
) {
    // TODO Remove this
    let (project_name, new_operation) = request.into_inner();

    let (operation, exists) = {
        let read_db = db.read().unwrap();
        match read_db.get(&project_name) {
            Some(project) => ((*project).operation.clone(), true),
            None => (Arc::new(Operation::Idle), false),
        }
    };

    let new_operation = match *operation {
        Operation::Idle => Arc::new(new_operation),
        _ => operation,
    };

    if exists != true {
        let mut write_db = db.write().unwrap();
        write_db.insert(
            project_name.clone(),
            Project {
                operation: new_operation,
                participants: AtomicUsize::new(0),
                cache: RwLock::new(HashMap::new()),
            },
        );
    } else {
        let mut write_db = db.write().unwrap();
        match write_db.get_mut(&project_name) {
            Some(project) => project.operation = new_operation,
            None => unreachable!(),
        }
    };
}

#[post("/get-operation", format = "json", data = "<request>")]
fn get_operation(
    db: State<RwLock<HashMap<String, Project>>>,
    request: Json<(String, usize)>,
) -> Json<Operation> {
    let (project_name, _) = request.into_inner();

    let read_db = db.read().unwrap();

    // TODO How should we handle the case of the missing operation?
    // Probably want more error states here
    let operation = match read_db.get(&project_name) {
        Some(op) => (*op.operation).clone(),
        None => Operation::Idle,
    };

    Json(operation)
}

// TODO Maybe we should be checking how many participants there are?
// Figure out how to best share that state between threads
#[post("/end-operation", format = "json", data = "<request>")]
fn end_operation(db: State<RwLock<HashMap<String, Project>>>, request: Json<(String, Operation)>) {
    let (project_name, _) = request.into_inner();

    // Reset project operation
    db.write()
        .unwrap()
        .get_mut(&project_name)
        .unwrap()
        .operation = Arc::new(Operation::Idle);
    // Reset participant number
    db.write()
        .unwrap()
        .get_mut(&project_name)
        .unwrap()
        .participants
        .store(0, Ordering::SeqCst);
}

#[post("/clear", format = "json", data = "<request>")]
fn clear(db: State<RwLock<HashMap<String, Project>>>, request: Json<(String, usize)>) {
    let (project_name, _) = request.into_inner();
    db.write()
        .unwrap()
        .get_mut(&project_name)
        .unwrap()
        .cache
        .write()
        .unwrap()
        .clear()
}

#[post("/get", format = "json", data = "<request>")]
fn get(
    db_mtx: State<RwLock<HashMap<String, Project>>>,
    request: Json<(String, Index)>,
) -> Json<Result<Entry, ()>> {
    let (project_name, index) = request.into_inner();

    println!("Index: {:?}", index);
    // TODO I don't like holding the lock for so long but it seems necessary
    let hm = db_mtx.read().unwrap();
    let project = hm.get(&project_name).unwrap();

    let read_db = project.cache.read().unwrap();

    match read_db.get(&index.key) {
        Some(v) => {
            let entry = Entry {
                key: index.key,
                value: v.clone().to_string(),
            };
            Json(Ok(entry))
        }
        None => Json(Err(())),
    }
}

#[post("/set", format = "json", data = "<request>")]
fn set(
    db_mtx: State<RwLock<HashMap<String, Project>>>,
    request: Json<(String, Entry)>,
) -> Json<Result<(), ()>> {
    let (project_name, entry) = request.into_inner();

    println!("Getting index: {:?}", entry);
    let hm = db_mtx.write().unwrap();
    let project = hm.get(&project_name).unwrap();
    let mut project_cache = project.cache.write().unwrap();

    project_cache.insert(entry.key.clone(), entry.value.clone());
    Json(Ok(()))
}

#[post("/signupkeygen", format = "json", data = "<request>")]
fn signup_keygen(
    db_mtx: State<RwLock<HashMap<String, Project>>>,
    request: Json<(String, String)>,
) -> Json<Result<PartySignup, ()>> {
    let (project_name, _) = request.into_inner();

    let hm = db_mtx.read().unwrap();
    let project = hm.get(&project_name).unwrap();

    let op = &project.operation;
    let parties = match **op {
        Operation::KeyGen { participants, .. } => participants,
        _ => return Json(Err(())),
    } as usize;
    let participants = &project.participants;

    let res = if participants.load(Ordering::SeqCst) < parties {
        let index = participants.fetch_add(1, Ordering::SeqCst) + 1;

        Ok(PartySignup {
            number: index as u16,
        })
    } else {
        Err(())
    };

    Json(res)
}

#[post("/signupsign", format = "json", data = "<request>")]
fn signup_sign(db_mtx: State<RwLock<HashMap<String, Project>>>, request: Json<(String, String)>) -> Json<Result<PartySignup, ()>> {
    let (project_name, _) = request.into_inner();

    let hm = db_mtx.read().unwrap();
    let project = hm.get(&project_name).unwrap();

    let op = &project.operation;
    let threshold = match **op {
        Operation::SignTag {
            participants: _,
            threshold,
            ..
        } => threshold,
        Operation::SignKey {
            participants: _,
            threshold,
            ..
        } => threshold,
        _ => {
            println!("op: {:?}", op); 
            panic!("Trying to register for signatures when everything is not yet done!");
        }
    } as usize;

    let participants = &project.participants;

    println!("participants: {}", participants.load(Ordering::SeqCst));
    let res = if participants.load(Ordering::SeqCst) < threshold + 1 {
        let index = participants.fetch_add(1, Ordering::SeqCst) + 1;

        Ok(PartySignup {
            number: index as u16,
        })
    } else {
        Err(())
    };

    println!("res: {:?}", res);

    Json(res)
}

struct Project {
    operation: Arc<Operation>,
    participants: AtomicUsize,
    cache: RwLock<HashMap<Key, String>>,
}

fn main() {
    let projects: HashMap<String, Project> = HashMap::with_capacity(1);
    let db_mtx = RwLock::new(projects);

    rocket::ignite()
        .mount(
            "/",
            routes![
                get,
                set,
                signup_keygen,
                signup_sign,
                start_operation,
                end_operation,
                get_operation,
                clear
            ],
        )
        .manage(db_mtx)
        .launch();
}
