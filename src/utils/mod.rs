use core::fmt;

pub mod vector_map;

// TODO: just using bindgen is probably the simpler thing to do at this point
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
pub struct __IncompleteArrayField<T>(core::marker::PhantomData<T>);
impl<T> __IncompleteArrayField<T> {
    #[inline]
    pub unsafe fn as_ptr(&self) -> *const T {
	self as *const Self as *const T
    }
    #[inline]
    #[allow(dead_code)]
    pub unsafe fn as_mut_ptr(&mut self) -> *mut T {
	self as *mut Self as *mut T
    }
    #[inline]
    pub unsafe fn as_slice(&self, len: usize) -> &[T] {
        alloc::slice::from_raw_parts(self.as_ptr(), len)
    }
    #[inline]
    #[allow(dead_code)]
    pub unsafe fn as_mut_slice(&mut self, len: usize) -> &mut [T] {
        alloc::slice::from_raw_parts_mut(self.as_mut_ptr(), len)
    }
}
impl<T> fmt::Debug for __IncompleteArrayField<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.write_str("__IncompleteArrayField")
    }
}
impl<T> Clone for __IncompleteArrayField<T> {
    #[inline]
    fn clone(&self) -> Self {
	//        Self::new()
	*self
    }
}
impl<T> Copy for __IncompleteArrayField<T> {}
