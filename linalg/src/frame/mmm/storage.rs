use std::ffi::c_void;
use std::fmt;
use std::fmt::Debug;
use tract_data::internal::*;

#[derive(PartialEq, Clone, Debug, Hash)]
pub enum MatrixStoreSpec {
    View { axes: Option<(usize, usize)> },
    Packed { panel_len: usize },
    Strides { row_byte_stride: isize, col_byte_stride: isize },
    OffsetsAndPtrs { row_byte_offsets: Vec<isize>, col_byte_offsets: Vec<isize>, nr: usize },
    VecStride { byte_stride: isize, mr: usize, nr: usize },
}

impl MatrixStoreSpec {
    pub unsafe fn wrap<'t>(&self, tensor: &'t TensorView) -> MatrixStore<'_, 't> {
        MatrixStore::new(self, tensor)
    }
}

impl fmt::Display for MatrixStoreSpec {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MatrixStoreSpec::View { .. } => write!(fmt, "ViewAxis"),
            MatrixStoreSpec::Packed { .. } => write!(fmt, "Packed"),
            MatrixStoreSpec::Strides { .. } => write!(fmt, "Strides"),
            MatrixStoreSpec::OffsetsAndPtrs { .. } => write!(fmt, "OffsetsAndPtrs"),
            MatrixStoreSpec::VecStride { .. } => write!(fmt, "VecStrides"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MatrixStore<'s, 't> {
    pub(crate) spec: &'s MatrixStoreSpec,
    pub(crate) tensor: &'t TensorView<'t>,
    pub(crate) col_ptrs: Option<Vec<*const u8>>,
}

impl<'s, 't> MatrixStore<'s, 't> {
    unsafe fn new(spec: &'s MatrixStoreSpec, tensor: &'t TensorView) -> MatrixStore<'s, 't> {
        let mut store = MatrixStore { spec, tensor, col_ptrs: None };
        if let MatrixStoreSpec::OffsetsAndPtrs { col_byte_offsets, .. } = spec {
            let ptr = tensor.as_ptr_unchecked::<u8>();
            let col_ptrs: Vec<_> =
                col_byte_offsets.iter().map(|&i| (ptr as *const u8).offset(i) as _).collect();
            store.col_ptrs = Some(col_ptrs);
        }
        store
    }

    pub(super) unsafe fn panel_a(&self, i: usize) -> PanelStore {
        let ptr = self.tensor.as_ptr_unchecked::<u8>();
        let dt = self.tensor.datum_type();
        match self.spec {
            MatrixStoreSpec::Packed { panel_len } => {
                PanelStore::Packed { ptr: ptr.offset((panel_len * i * dt.size_of()) as isize) as _ }
            }
            _ => unimplemented!(),
        }
    }

    pub(super) unsafe fn panel_b(&self, nr: usize, i: usize, n: usize) -> PanelStore {
        let ptr = self.tensor.as_ptr_unchecked::<u8>();
        let dt = self.tensor.datum_type();
        match self.spec {
            MatrixStoreSpec::Packed { panel_len } => {
                if nr * i + 1 == n {
                    PanelStore::VecStride {
                        ptr: ptr.offset((panel_len * i * dt.size_of()) as isize) as _,
                        byte_stride: (nr * dt.size_of()) as isize,
                        item_size: dt.size_of(),
                    }
                } else {
                    PanelStore::Packed {
                        ptr: ptr.offset((panel_len * i * dt.size_of()) as isize) as _,
                    }
                }
            }
            MatrixStoreSpec::OffsetsAndPtrs { row_byte_offsets, nr, .. } => {
                PanelStore::OffsetsAndPtrs {
                    row_byte_offsets: row_byte_offsets.as_ptr(),
                    col_ptrs: self.col_ptrs.as_ref().unwrap().as_ptr().offset((nr * i) as isize)
                        as _,
                }
            }
            MatrixStoreSpec::VecStride { byte_stride, .. } => PanelStore::VecStride {
                ptr: ptr as _,
                byte_stride: *byte_stride,
                item_size: dt.size_of(),
            },
            _ => unimplemented!(),
        }
    }

    #[inline]
    unsafe fn strides(&self) -> (isize, isize) {
        match self.spec {
            MatrixStoreSpec::View { axes } => {
                let (m_axis, n_axis) = if let Some(axes) = axes {
                    axes.clone()
                } else {
                    let rank = self.tensor.rank();
                    (rank - 2, rank - 1)
                };
                let tensor_strides = self.tensor.strides();
                let row_byte_stride = tensor_strides.get_unchecked(m_axis)
                    * self.tensor.datum_type().size_of() as isize;
                let col_byte_stride = tensor_strides.get_unchecked(n_axis)
                    * self.tensor.datum_type().size_of() as isize;
                (row_byte_stride, col_byte_stride)
            }
            MatrixStoreSpec::Strides { row_byte_stride, col_byte_stride } => {
                (*row_byte_stride, *col_byte_stride)
            }
            MatrixStoreSpec::VecStride { byte_stride, .. } => (*byte_stride, 0),
            _ => panic!(),
        }
    }

    pub(super) unsafe fn tile_c(
        &self,
        down: usize,
        right: usize,
        mr: usize,
        nr: usize,
    ) -> PanelStore {
        let (down, right, mr, nr) = (down as isize, right as isize, mr as isize, nr as isize);
        match self.spec {
            MatrixStoreSpec::Strides { .. } | MatrixStoreSpec::View { .. } => {
                let ptr = self.tensor.as_ptr_unchecked::<u8>();
                let (row_byte_stride, col_byte_stride) = self.strides();
                PanelStore::Strides {
                    ptr: ptr.offset(row_byte_stride * down * mr + col_byte_stride * right * nr)
                        as *mut _,
                    row_byte_stride,
                    col_byte_stride,
                    item_size: self.tensor.datum_type().size_of(),
                }
            }
            MatrixStoreSpec::VecStride { .. } => {
                let ptr = self.tensor.as_ptr_unchecked::<u8>();
                let (row_byte_stride, _col_byte_stride) = self.strides();
                PanelStore::VecStride {
                    ptr: ptr.offset(row_byte_stride * down * mr) as *mut _,
                    byte_stride: row_byte_stride,
                    item_size: self.tensor.datum_type().size_of(),
                }
            }
            _ => unimplemented!(),
        }
    }

    pub(super) unsafe fn set_from_tile<T: Datum + Copy>(
        &mut self,
        down: usize,
        right: usize,
        height: usize,
        width: usize,
        tile: &TensorView,
        mr: usize,
        nr: usize,
    ) {
        let (row_byte_stride, col_byte_stride) = self.strides();
        let mut dst = self.tensor.as_ptr_unchecked::<u8>().offset(
            (row_byte_stride as usize * (down * mr) + col_byte_stride as usize * (right * nr))
                as isize,
        );
        match self.spec {
            MatrixStoreSpec::Strides { .. } | MatrixStoreSpec::View { .. } => {
                for y in 0..height {
                    let mut row_dst = dst;
                    for x in 0..width {
                        let value = *tile.as_ptr_unchecked::<T>().offset((y + x * mr) as isize);
                        *(row_dst as *mut T) = value;
                        row_dst = row_dst.offset(col_byte_stride);
                    }
                    dst = dst.offset(row_byte_stride);
                }
            }
            MatrixStoreSpec::VecStride { .. } => {
                for y in 0..height {
                    let value = *tile.as_ptr_unchecked::<T>().offset(y as isize);
                    *(dst as *mut T) = value;
                    dst = dst.offset(row_byte_stride);
                }
            }
            _ => unimplemented!(),
        }
    }
}

#[repr(C, usize)]
#[derive(PartialEq, Copy, Clone, Debug)]
pub enum PanelStore {
    Strides { ptr: *mut c_void, row_byte_stride: isize, col_byte_stride: isize, item_size: usize },
    Packed { ptr: *const c_void },
    OffsetsAndPtrs { row_byte_offsets: *const isize, col_ptrs: *const *const c_void },
    VecStride { ptr: *const c_void, byte_stride: isize, item_size: usize },
}
