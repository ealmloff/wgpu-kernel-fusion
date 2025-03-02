use wgpu::CommandEncoder;

use crate::{
    ElementWiseFunction, UntypedElementWiseKernel, UntypedPairWiseKernel, UntypedReduceKernel,
    element_wise, matmul::UntypedMatMul, resize::UntypedResizeKernel,
    slice_assign::UntypedSliceAssignKernel, tensor::TensorData,
};

use super::{
    AnyComputeKey, ComputeGraphInner, ElementWiseComputeNodeKey, MatMulComputeNodeKey,
    PairWiseComputeNodeKey, ReduceComputeNodeKey, ResizeComputeNodeKey, SliceAssignComputeNodeKey,
    MapLayoutComputeNodeKey, TensorComputeNodeKey,
};

impl ComputeGraphInner {
    pub(crate) fn resolve(
        &self,
        key: AnyComputeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let graph = self.graphvis(key);
        println!("{graph}");
        match key {
            AnyComputeKey::ElementWiseComputeNodeKey(element_wise_compute_node_key) => {
                self.resolve_element_wise(element_wise_compute_node_key, command_encoder)
            }
            AnyComputeKey::PairWiseComputeNodeKey(pair_wise_compute_node_key) => {
                self.resolve_pair_wise(pair_wise_compute_node_key, command_encoder)
            }
            AnyComputeKey::MatMulComputeNodeKey(mat_mul_compute_node_key) => {
                self.resolve_mat_mul(mat_mul_compute_node_key, command_encoder)
            }
            AnyComputeKey::ReduceComputeNodeKey(reduce_compute_node_key) => {
                self.resolve_reduce(reduce_compute_node_key, command_encoder)
            }
            AnyComputeKey::TensorComputeNodeKey(tensor_compute_node_key) => {
                self.resolve_tensor(tensor_compute_node_key, command_encoder)
            }
            AnyComputeKey::MapLayoutComputeNodeKey(slice_compute_node_key) => {
                self.resolve_slice(slice_compute_node_key, command_encoder)
            }
            AnyComputeKey::ResizeComputeNodeKey(resize_compute_node_key) => {
                self.resolve_resize(resize_compute_node_key, command_encoder)
            }
            AnyComputeKey::SliceAssignComputeNodeKey(slice_assign_compute_node_key) => {
                self.resolve_slice_assign(slice_assign_compute_node_key, command_encoder)
            }
        }
    }

    fn collect_element_wise_ops(
        &self,
        key: ElementWiseComputeNodeKey,
    ) -> (Vec<ElementWiseFunction>, AnyComputeKey) {
        let mut functions = Vec::new();
        let mut current_key = AnyComputeKey::ElementWiseComputeNodeKey(key);
        while let AnyComputeKey::ElementWiseComputeNodeKey(key) = current_key {
            let operation = self.element_wise.get(&key).unwrap();
            functions.push(operation.function.clone());
            current_key = operation.value;
        }
        (functions, current_key)
    }

    fn resolve_element_wise(
        &self,
        key: ElementWiseComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        // First collect all element wise ops in this chain
        let (functions, input) = self.collect_element_wise_ops(key);

        // Merge into the output of the reduce kernel if possible
        if let AnyComputeKey::ReduceComputeNodeKey(key) = input {
            self.resolve_reduce_then(key, functions, command_encoder)
        }
        // Merge into the output of the pair wise kernel if possible
        else if let AnyComputeKey::PairWiseComputeNodeKey(key) = input {
            self.resolve_pair_wise_then(key, functions, command_encoder)
        } else {
            let input = self.resolve(input, &mut *command_encoder);
            let kernel = UntypedElementWiseKernel::new(functions, input.datatype());
            kernel
                .run_with_query(&input, None, command_encoder)
                .unwrap_or(input)
        }
    }

    fn resolve_pair_wise(
        &self,
        key: PairWiseComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        self.resolve_pair_wise_then(key, Vec::new(), command_encoder)
    }

    fn resolve_pair_wise_then(
        &self,
        key: PairWiseComputeNodeKey,
        then: Vec<ElementWiseFunction>,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let operation = self.pair_wise.get(&key).unwrap();

        let mut first_input = operation.first;
        let first_pre_element_wise =
            if let AnyComputeKey::ElementWiseComputeNodeKey(key) = operation.first {
                let (functions, element_wise_input) = self.collect_element_wise_ops(key);
                first_input = element_wise_input;
                functions
            } else {
                Vec::new()
            };
        let mut second_input = operation.second;
        let second_pre_element_wise =
            if let AnyComputeKey::ElementWiseComputeNodeKey(key) = operation.second {
                let (functions, element_wise_input) = self.collect_element_wise_ops(key);
                second_input = element_wise_input;
                functions
            } else {
                Vec::new()
            };

        let first = self.resolve(first_input, &mut *command_encoder);
        let second = self.resolve(second_input, &mut *command_encoder);
        let mut kernel = UntypedPairWiseKernel::new(operation.function.clone(), first.datatype());
        let first_pre = UntypedElementWiseKernel::new(first_pre_element_wise, first.datatype());
        let second_pre = UntypedElementWiseKernel::new(second_pre_element_wise, first.datatype());
        let pre_element_wise_output = first_pre.out_datatype();
        kernel.set_pre_element_wise([first_pre, second_pre]);
        kernel.set_post_element_wise(UntypedElementWiseKernel::new(then, pre_element_wise_output));
        kernel
            .run_with_query(&first, &second, None, command_encoder)
            .unwrap_or(second)
    }

    fn resolve_mat_mul(
        &self,
        key: MatMulComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let operation = self.mat_mul.get(&key).unwrap();

        let first = self.resolve(operation.first, &mut *command_encoder);
        let second = self.resolve(operation.second, &mut *command_encoder);
        let kernel = UntypedMatMul::new(first.datatype());
        kernel.run_with_query(&first, &second, None, command_encoder)
    }

    fn resolve_reduce(
        &self,
        key: ReduceComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        self.resolve_reduce_then(key, Vec::new(), command_encoder)
    }

    fn resolve_reduce_then(
        &self,
        key: ReduceComputeNodeKey,
        then: Vec<ElementWiseFunction>,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let operation = self.reduce.get(&key).unwrap();
        let mut input = operation.value;

        let element_wise_before =
            if let AnyComputeKey::ElementWiseComputeNodeKey(key) = operation.value {
                let (functions, element_wise_input) = self.collect_element_wise_ops(key);
                input = element_wise_input;
                functions
            } else {
                Vec::new()
            };

        let input = self.resolve(input, &mut *command_encoder);
        let mut kernel = UntypedReduceKernel::new(operation.function.clone(), input.datatype());
        let element_wise_before =
            element_wise::UntypedElementWiseKernel::new(element_wise_before, input.datatype());
        let element_wise_after =
            element_wise::UntypedElementWiseKernel::new(then, element_wise_before.out_datatype());
        kernel.set_post_element_wise(element_wise_after);
        kernel.set_pre_element_wise(element_wise_before);
        kernel.run_with_query(&input, operation.axis, None, command_encoder)
    }

    fn resolve_slice(
        &self,
        key: MapLayoutComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let operation = self.map_layout.get(&key).unwrap();
        let input = self.resolve(operation.input, &mut *command_encoder);

        operation.run(&input)
    }

    fn resolve_resize(
        &self,
        key: ResizeComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let operation = self.resize.get(&key).unwrap();
        let input = self.resolve(operation.input, &mut *command_encoder);
        let kernel = UntypedResizeKernel::new(&operation.new_shape, &operation.fill_shape);

        kernel.run_with_query(&input, None, command_encoder)
    }

    fn resolve_slice_assign(
        &self,
        key: SliceAssignComputeNodeKey,
        command_encoder: &mut CommandEncoder,
    ) -> TensorData {
        let operation = self.slice_assign.get(&key).unwrap();
        let input = self.resolve(operation.input, &mut *command_encoder);
        let value = self.resolve(operation.value, &mut *command_encoder);
        let kernel = UntypedSliceAssignKernel::new(&operation.slices);

        kernel.run_with_query(&input, &value, None, command_encoder)
    }

    fn resolve_tensor(&self, key: TensorComputeNodeKey, _: &mut CommandEncoder) -> TensorData {
        self.tensor.get(&key).unwrap().clone()
    }
}
