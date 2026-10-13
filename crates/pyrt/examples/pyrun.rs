//! Run a Python file with the embedded interpreter (testing aid).
//! usage: pyrun MAIN.py [--module NAME=FILE.py]...
//! The `testio` module offers read(path) / write(path, text) for test harnesses.
use curust_pyrt::{Flow, Host, Interp, Value, R};
use std::rc::Rc;

struct StdoutHost;
impl Host for StdoutHost {
    fn import(&self, _i: &mut Interp, name: &str) -> Option<R<Value>> {
        if name == "testio" {
            let read: curust_pyrt::value::BuiltinFn = |i, a, _kw| {
                let p = i.to_str(&a[0])?;
                Ok(Value::Str(std::fs::read_to_string(&p).map_err(|e| i.exc("OSError", e.to_string()))?.into()))
            };
            let write: curust_pyrt::value::BuiltinFn = |i, a, _kw| {
                let p = i.to_str(&a[0])?;
                let t = i.to_str(&a[1])?;
                std::fs::write(&p, t).map_err(|e| i.exc("OSError", e.to_string()))?;
                Ok(Value::None)
            };
            return Some(Ok(curust_pyrt::modules::make_module("testio", vec![("read", curust_pyrt::builtins::builtin("read", read)), ("write", curust_pyrt::builtins::builtin("write", write))])));
        }
        None
    }
    fn print(&self, text: &str) {
        println!("{}", text);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: pyrun FILE.py [--module NAME=FILE.py]");
    let src = std::fs::read_to_string(path).expect("read");
    let mut i = Interp::new();
    i.host = Some(Rc::new(StdoutHost));
    let mut k = 2;
    while k < args.len() {
        if args[k] == "--module" {
            let (n, f) = args[k + 1].split_once('=').expect("NAME=FILE");
            i.sources.insert(n.into(), std::fs::read_to_string(f).expect("module source"));
            k += 2;
        } else {
            k += 1;
        }
    }
    let t = std::time::Instant::now();
    match i.run_module("__main__", &src) {
        Ok(_) => {}
        Err(Flow::Exc(e)) => {
            let ty = i.exc_type_name(&e);
            let msg = i.exc_message(&e);
            let line = match &e {
                Value::Instance(inst) => match inst.dict.borrow().get("__line__") {
                    Some(Value::Int(n)) => n.to_string(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            println!("EXCEPTION {}: {} (line {})", ty, msg, line);
        }
        Err(_) => {}
    }
    if std::env::var("PYRUN_TIME").is_ok() {
        eprintln!("time: {:?}", t.elapsed());
    }
}
