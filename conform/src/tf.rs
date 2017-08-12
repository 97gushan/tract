use std::{fs, path};

use tensorflow::Graph;
use tensorflow::Session;
use tensorflow::StepWithGraph;
use tensorflow::Tensor;

use ndarray::ArrayD;
use tfdeploy::Matrix;
use errors::*;

pub struct Tensorflow {
    session: Session,
    graph: Graph,
}

pub fn build<P: AsRef<path::Path>>(p: P) -> Result<Box<::TfExecutor>> {
    use std::io::Read;
    let mut graph = Graph::new();
    let mut model = vec![];
    fs::File::open(p)?.read_to_end(&mut model)?;
    graph.import_graph_def(
        &*model,
        &::tensorflow::ImportGraphDefOptions::new(),
    )?;
    let session = Session::new(&::tensorflow::SessionOptions::new(), &graph)?;
    Ok(Box::new(Tensorflow { session, graph }))
}

enum TensorHolder {
    F32(Tensor<f32>),
    I32(Tensor<i32>),
    U8(Tensor<u8>),
}

impl TensorHolder {
    fn to_tensor<T: ::tensorflow::TensorType>(m: ArrayD<T>) -> Tensor<T> {
        let dims: Vec<u64> = m.shape().iter().map(|d| *d as _).collect();
        let mut tensor = Tensor::<T>::new(&*dims);
        tensor.copy_from_slice(m.as_slice().unwrap());
        tensor
    }
}

impl From<Matrix> for TensorHolder {
    fn from(m: Matrix) -> TensorHolder {
        match m {
            Matrix::F32(a) => TensorHolder::F32(Self::to_tensor(a)),
            Matrix::I32(a) => TensorHolder::I32(Self::to_tensor(a)),
            Matrix::U8(a) => TensorHolder::U8(Self::to_tensor(a)),
        }
    }
}

fn tensor_to_matrix<T: ::tensorflow::TensorType>(tensor: &Tensor<T>) -> Result<ArrayD<T>> {
    let shape: Vec<usize> = tensor.dims().iter().map(|d| *d as _).collect();
    Ok(::ndarray::Array::from_iter(tensor.iter().cloned())
        .into_shape(shape)?)
}

impl ::TfExecutor for Tensorflow {
    fn run(&mut self, inputs: Vec<(&str, Matrix)>, output_name: &str) -> Result<Vec<Matrix>> {
        use tensorflow::DataType;
        let tensors: Vec<(&str, TensorHolder)> = inputs
            .into_iter()
            .map(|(name, mat)| (name, mat.into()))
            .collect();
        let mut step = StepWithGraph::new();
        for t in &tensors {
            let op = self.graph.operation_by_name_required(t.0)?;
            match t.1 {
                TensorHolder::F32(ref it) => step.add_input(&op, 0, &it),
                TensorHolder::I32(ref it) => step.add_input(&op, 0, &it),
                TensorHolder::U8(ref it) => step.add_input(&op, 0, &it),
            }
        }
        let output = step.request_output(&self.graph.operation_by_name_required(output_name)?, 0);
        self.session.run(&mut step)?;
        let matrix = match step.output_data_type(0).unwrap() {
            DataType::Float => Matrix::F32(tensor_to_matrix(&step.take_output(output)?)?),
            DataType::UInt8 => Matrix::U8(tensor_to_matrix(&step.take_output(output)?)?),
            _ => unimplemented!(),
        };
        Ok(vec![matrix])
    }
}
