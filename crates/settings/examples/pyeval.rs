//! Differential-testing driver: reads one expression per line (optionally `name=value;...|expr`
//! with literal locals) and prints `str(result)`, `<ExceptionType>` or `<invalid>`.
use curust_settings::py::{literal_eval, Env, Function, PyErr, PyResult, Value};
use std::io::BufRead;
use std::rc::Rc;

struct Locals(Vec<(String, Value)>);
impl Env for Locals {
    fn local(&self, name: &str) -> Option<Value> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }
    fn global(&self, _: &str) -> Option<Value> {
        None
    }
    fn call_operator(&self, _: &str, _: &[Value], _: &[(Rc<str>, Value)]) -> PyResult<Value> {
        Err(PyErr::type_err("no operators"))
    }
}

fn main() {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let (locals, expr) = match line.split_once("|||") {
            Some((l, e)) => (l.to_string(), e.to_string()),
            None => (String::new(), line.clone()),
        };
        let mut vars = Vec::new();
        for kv in locals.split(";;").filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap();
            vars.push((k.to_string(), literal_eval(v).unwrap()));
        }
        let f = Function::new(&expr);
        let out = match f.try_evaluate(&Locals(vars)) {
            None => "<invalid>".to_string(),
            Some(Ok(v)) => v.py_str(),
            Some(Err(e)) => format!("<{}>", e.kind),
        };
        println!("{}", out.replace('\n', "\\n"));
    }
}
