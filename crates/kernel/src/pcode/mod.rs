//! Pseudo-code executor for safe script execution
//!
//! Provides:
//! - Sandboxed script execution
//! - Limited instruction set
//! - Resource limits
//! - Step-by-step debugging

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{info, trace};

/// PCode value types
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Integer(i) => write!(f, "{}", i),
            Value::Float(fl) => write!(f, "{}", fl),
            Value::String(s) => write!(f, "\"{}\"", s),
            Value::List(l) => write!(f, "{:?}", l),
            Value::Map(m) => write!(f, "{:?}", m),
        }
    }
}

/// PCode instruction
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    /// Load constant
    LoadConst(usize),
    /// Load variable
    LoadVar(String),
    /// Store variable
    StoreVar(String),
    /// Binary operation
    Binary(Op),
    /// Unary operation
    Unary(UnaryOp),
    /// Jump
    Jump(usize),
    /// Jump if false
    JumpIfFalse(usize),
    /// Call function
    Call(String, usize), // name, arg_count
    /// Return value
    Return,
    /// Pop stack
    Pop,
    /// Dup top
    Dup,
    /// No operation
    Nop,
}

/// Binary operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Unary operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

/// PCode function
#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<String>,
    pub instructions: Vec<Instruction>,
    pub constants: Vec<Value>,
}

/// Execution context
pub struct Context {
    /// Local variables
    pub variables: HashMap<String, Value>,
    /// Call stack depth
    pub call_depth: usize,
    /// Instruction counter
    pub instruction_count: usize,
}

impl Context {
    /// Create new context
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            call_depth: 0,
            instruction_count: 0,
        }
    }

    /// Get variable
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.variables.get(name)
    }

    /// Set variable
    pub fn set(&mut self, name: impl Into<String>, value: Value) {
        self.variables.insert(name.into(), value);
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

/// Execution configuration
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Maximum stack size
    pub max_stack_size: usize,
    /// Maximum call depth
    pub max_call_depth: usize,
    /// Maximum instructions to execute
    pub max_instructions: usize,
    /// Enable debugging
    pub debug: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_stack_size: 1000,
            max_call_depth: 100,
            max_instructions: 10000,
            debug: false,
        }
    }
}

/// PCode VM
pub struct VM {
    config: ExecutionConfig,
    functions: HashMap<String, Function>,
    stack: Vec<Value>,
    context: Context,
    builtin: HashMap<String, Arc<dyn Fn(&[Value]) -> anyhow::Result<Value> + Send + Sync>>,
}

impl VM {
    /// Create a new VM
    pub fn new(config: ExecutionConfig) -> Self {
        let mut vm = Self {
            config,
            functions: HashMap::new(),
            stack: Vec::new(),
            context: Context::new(),
            builtin: HashMap::new(),
        };
        
        vm.register_builtins();
        vm
    }

    /// Register built-in functions
    fn register_builtins(&mut self) {
        self.builtin.insert(
            "print".to_string(),
            Arc::new(|args| {
                let output = args
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("{}", output);
                Ok(Value::Null)
            }),
        );

        self.builtin.insert(
            "len".to_string(),
            Arc::new(|args| {
                if let Some(arg) = args.first() {
                    match arg {
                        Value::String(s) => Ok(Value::Integer(s.len() as i64)),
                        Value::List(l) => Ok(Value::Integer(l.len() as i64)),
                        Value::Map(m) => Ok(Value::Integer(m.len() as i64)),
                        _ => Ok(Value::Integer(0)),
                    }
                } else {
                    Ok(Value::Integer(0))
                }
            }),
        );

        self.builtin.insert(
            "type".to_string(),
            Arc::new(|args| {
                if let Some(arg) = args.first() {
                    let type_name = match arg {
                        Value::Null => "null",
                        Value::Bool(_) => "bool",
                        Value::Integer(_) => "integer",
                        Value::Float(_) => "float",
                        Value::String(_) => "string",
                        Value::List(_) => "list",
                        Value::Map(_) => "map",
                    };
                    Ok(Value::String(type_name.to_string()))
                } else {
                    Ok(Value::String("null".to_string()))
                }
            }),
        );
    }

    /// Register a function
    pub fn register_function(&mut self, function: Function) {
        info!("Registering pcode function: {}", function.name);
        self.functions.insert(function.name.clone(), function);
    }

    /// Push value onto stack
    fn push(&mut self, value: Value) -> anyhow::Result<()> {
        if self.stack.len() >= self.config.max_stack_size {
            anyhow::bail!("Stack overflow");
        }
        self.stack.push(value);
        Ok(())
    }

    /// Pop value from stack
    fn pop(&mut self) -> anyhow::Result<Value> {
        self.stack.pop().ok_or_else(|| anyhow::anyhow!("Stack underflow"))
    }

    /// Peek at top of stack
    fn peek(&self) -> anyhow::Result<&Value> {
        self.stack.last().ok_or_else(|| anyhow::anyhow!("Stack empty"))
    }

    /// Execute a function
    pub fn execute(&mut self, function_name: &str, args: Vec<Value>) -> anyhow::Result<Value> {
        let func = self.functions
            .get(function_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Function not found: {}", function_name))?;

        // Set up arguments
        for (i, param) in func.params.iter().enumerate() {
            let value = args.get(i).cloned().unwrap_or(Value::Null);
            self.context.set(param.clone(), value);
        }

        self.context.call_depth += 1;
        if self.context.call_depth > self.config.max_call_depth {
            anyhow::bail!("Call depth exceeded");
        }

        let result = self.run_instructions(&func);

        self.context.call_depth -= 1;
        result
    }

    /// Run instructions
    fn run_instructions(&mut self, func: &Function) -> anyhow::Result<Value> {
        let mut pc = 0;
        let instructions = &func.instructions;

        while pc < instructions.len() {
            self.context.instruction_count += 1;
            
            if self.context.instruction_count > self.config.max_instructions {
                anyhow::bail!("Instruction limit exceeded");
            }

            if self.config.debug {
                trace!("PC: {} Instruction: {:?}", pc, instructions[pc]);
            }

            match &instructions[pc] {
                Instruction::LoadConst(idx) => {
                    let value = func.constants.get(*idx)
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.push(value)?;
                    pc += 1;
                }

                Instruction::LoadVar(name) => {
                    let value = self.context.get(name)
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.push(value)?;
                    pc += 1;
                }

                Instruction::StoreVar(name) => {
                    let value = self.pop()?;
                    self.context.set(name.clone(), value);
                    pc += 1;
                }

                Instruction::Binary(op) => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let result = self.execute_binary(*op, left, right)?;
                    self.push(result)?;
                    pc += 1;
                }

                Instruction::Unary(op) => {
                    let val = self.pop()?;
                    let result = self.execute_unary(*op, val)?;
                    self.push(result)?;
                    pc += 1;
                }

                Instruction::Jump(target) => {
                    pc = *target;
                }

                Instruction::JumpIfFalse(target) => {
                    let cond = self.pop()?;
                    if !self.is_truthy(&cond) {
                        pc = *target;
                    } else {
                        pc += 1;
                    }
                }

                Instruction::Call(name, arg_count) => {
                    let mut args = Vec::new();
                    for _ in 0..*arg_count {
                        args.push(self.pop()?);
                    }
                    args.reverse();

                    let result = if let Some(builtin) = self.builtin.get(name) {
                        builtin(&args)?
                    } else {
                        self.execute(name, args)?
                    };
                    
                    self.push(result)?;
                    pc += 1;
                }

                Instruction::Return => {
                    return self.pop();
                }

                Instruction::Pop => {
                    self.pop()?;
                    pc += 1;
                }

                Instruction::Dup => {
                    let val = self.peek()?.clone();
                    self.push(val)?;
                    pc += 1;
                }

                Instruction::Nop => {
                    pc += 1;
                }
            }
        }

        // Return null if no explicit return
        Ok(Value::Null)
    }

    /// Execute binary operation
    fn execute_binary(&self, op: Op, left: Value, right: Value) -> anyhow::Result<Value> {
        match (op, left, right) {
            (Op::Add, Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a + b)),
            (Op::Add, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
            (Op::Add, Value::String(a), Value::String(b)) => Ok(Value::String(a + &b)),
            (Op::Sub, Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a - b)),
            (Op::Sub, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
            (Op::Mul, Value::Integer(a), Value::Integer(b)) => Ok(Value::Integer(a * b)),
            (Op::Mul, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
            (Op::Div, Value::Integer(a), Value::Integer(b)) => {
                if b == 0 {
                    anyhow::bail!("Division by zero");
                }
                Ok(Value::Integer(a / b))
            }
            (Op::Div, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
            (Op::Eq, a, b) => Ok(Value::Bool(a == b)),
            (Op::Ne, a, b) => Ok(Value::Bool(a != b)),
            (Op::Lt, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a < b)),
            (Op::Le, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a <= b)),
            (Op::Gt, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a > b)),
            (Op::Ge, Value::Integer(a), Value::Integer(b)) => Ok(Value::Bool(a >= b)),
            (Op::And, a, b) => Ok(Value::Bool(self.is_truthy(&a) && self.is_truthy(&b))),
            (Op::Or, a, b) => Ok(Value::Bool(self.is_truthy(&a) || self.is_truthy(&b))),
            _ => anyhow::bail!("Invalid binary operation"),
        }
    }

    /// Execute unary operation
    fn execute_unary(&self, op: UnaryOp, val: Value) -> anyhow::Result<Value> {
        match (op, val) {
            (UnaryOp::Not, v) => Ok(Value::Bool(!self.is_truthy(&v))),
            (UnaryOp::Neg, Value::Integer(i)) => Ok(Value::Integer(-i)),
            (UnaryOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
            _ => anyhow::bail!("Invalid unary operation"),
        }
    }

    /// Check if value is truthy
    fn is_truthy(&self, val: &Value) -> bool {
        match val {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Integer(0) => false,
            Value::Float(f) => *f != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Map(m) => !m.is_empty(),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_arithmetic() {
        let config = ExecutionConfig::default();
        let mut vm = VM::new(config);

        let func = Function {
            name: "test".to_string(),
            params: vec![],
            instructions: vec![
                Instruction::LoadConst(0), // 5
                Instruction::LoadConst(1), // 3
                Instruction::Binary(Op::Add),
                Instruction::Return,
            ],
            constants: vec![Value::Integer(5), Value::Integer(3)],
        };

        vm.register_function(func);
        let result = vm.execute("test", vec![]).unwrap();
        
        assert_eq!(result, Value::Integer(8));
    }

    #[test]
    fn test_vm_comparison() {
        let config = ExecutionConfig::default();
        let mut vm = VM::new(config);

        let func = Function {
            name: "test".to_string(),
            params: vec![],
            instructions: vec![
                Instruction::LoadConst(0), // 10
                Instruction::LoadConst(1), // 20
                Instruction::Binary(Op::Lt),
                Instruction::Return,
            ],
            constants: vec![Value::Integer(10), Value::Integer(20)],
        };

        vm.register_function(func);
        let result = vm.execute("test", vec![]).unwrap();
        
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn test_builtin_len() {
        let config = ExecutionConfig::default();
        let mut vm = VM::new(config);

        let func = Function {
            name: "test".to_string(),
            params: vec![],
            instructions: vec![
                Instruction::LoadConst(0), // "hello"
                Instruction::Call("len".to_string(), 1),
                Instruction::Return,
            ],
            constants: vec![Value::String("hello".to_string())],
        };

        vm.register_function(func);
        let result = vm.execute("test", vec![]).unwrap();
        
        assert_eq!(result, Value::Integer(5));
    }
}
