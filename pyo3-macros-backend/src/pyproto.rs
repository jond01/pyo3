// Copyright (c) 2017-present PyO3 Project and Contributors

use crate::defs;
use crate::method::{FnSpec, FnType};
use crate::proto_method::impl_method_proto;
use crate::pymethod;
use proc_macro2::{Span, TokenStream};
use quote::quote;
use quote::ToTokens;
use std::collections::HashSet;

pub fn build_py_proto(ast: &mut syn::ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &mut ast.trait_ {
        let proto = if let Some(segment) = path.segments.last() {
            match segment.ident.to_string().as_str() {
                "PyObjectProtocol" => &defs::OBJECT,
                "PyAsyncProtocol" => &defs::ASYNC,
                "PyMappingProtocol" => &defs::MAPPING,
                "PyIterProtocol" => &defs::ITER,
                "PyContextProtocol" => &defs::CONTEXT,
                "PySequenceProtocol" => &defs::SEQ,
                "PyNumberProtocol" => &defs::NUM,
                "PyDescrProtocol" => &defs::DESCR,
                "PyBufferProtocol" => &defs::BUFFER,
                "PyGCProtocol" => &defs::GC,
                _ => {
                    return Err(syn::Error::new_spanned(
                        path,
                        "#[pyproto] can not be used with this block",
                    ));
                }
            }
        } else {
            return Err(syn::Error::new_spanned(
                path,
                "#[pyproto] can only be used with protocol trait implementations",
            ));
        };

        let tokens = impl_proto_impl(&ast.self_ty, &mut ast.items, proto)?;

        // attach lifetime
        let mut seg = path.segments.pop().unwrap().into_value();
        seg.arguments = syn::PathArguments::AngleBracketed(syn::parse_quote! {<'p>});
        path.segments.push(seg);
        ast.generics.params = syn::parse_quote! {'p};

        Ok(tokens)
    } else {
        Err(syn::Error::new_spanned(
            ast,
            "#[pyproto] can only be used with protocol trait implementations",
        ))
    }
}

fn impl_proto_impl(
    ty: &syn::Type,
    impls: &mut Vec<syn::ImplItem>,
    proto: &defs::Proto,
) -> syn::Result<TokenStream> {
    let mut trait_impls = TokenStream::new();
    let mut py_methods = Vec::new();
    let mut method_names = HashSet::new();

    for iimpl in impls.iter_mut() {
        if let syn::ImplItem::Method(met) = iimpl {
            // impl Py~Protocol<'p> { type = ... }
            if let Some(m) = proto.get_proto(&met.sig.ident) {
                impl_method_proto(ty, &mut met.sig, m)?.to_tokens(&mut trait_impls);
                // Insert the method to the HashSet
                method_names.insert(met.sig.ident.to_string());
            }
            // Add non-slot methods to inventory like `#[pymethods]`
            if let Some(m) = proto.get_method(&met.sig.ident) {
                let name = &met.sig.ident;
                let fn_spec = FnSpec::parse(&met.sig, &mut met.attrs, false)?;

                let method = if let FnType::Fn(self_ty) = &fn_spec.tp {
                    pymethod::impl_proto_wrap(ty, &fn_spec, &self_ty)
                } else {
                    return Err(syn::Error::new_spanned(
                        &met.sig,
                        "Expected method with receiver for #[pyproto] method",
                    ));
                };

                let coexist = if m.can_coexist {
                    // We need METH_COEXIST here to prevent __add__  from overriding __radd__
                    quote!(pyo3::ffi::METH_COEXIST)
                } else {
                    quote!(0)
                };
                // TODO(kngwyu): Set ml_doc
                py_methods.push(quote! {
                    pyo3::class::PyMethodDefType::Method({
                        #method
                        pyo3::class::PyMethodDef::cfunction_with_keywords(
                            concat!(stringify!(#name), "\0"),
                            __wrap,
                            #coexist,
                            "\0"
                        )
                    })
                });
            }
        }
    }
    let normal_methods = submit_normal_methods(py_methods, ty);
    let protocol_methods = impl_proto_methods(method_names, ty, proto)?;
    Ok(quote! {
        #trait_impls
        #normal_methods
        #protocol_methods
    })
}

fn submit_normal_methods(py_methods: Vec<TokenStream>, ty: &syn::Type) -> TokenStream {
    if py_methods.is_empty() {
        return quote! {};
    }
    quote! {
        pyo3::inventory::submit! {
            #![crate = pyo3] {
                type Inventory = <#ty as pyo3::class::methods::HasMethodsInventory>::Methods;
                <Inventory as pyo3::class::methods::PyMethodsInventory>::new(vec![#(#py_methods),*])
            }
        }
    }
}

fn impl_proto_methods(
    method_names: HashSet<String>,
    ty: &syn::Type,
    proto: &defs::Proto,
) -> syn::Result<TokenStream> {
    let slots_trait: syn::Path = syn::parse_str(proto.slots_trait)?;
    let slots_trait_slots = syn::Ident::new(proto.slots_trait_slots, Span::call_site());

    if proto.name == "Buffer" {
        return Ok(quote! {
            impl #slots_trait<#ty> for pyo3::class::proto_methods::PyClassProtocols<#ty> {
                fn #slots_trait_slots(
                    self
                ) -> Option<&'static pyo3::class::proto_methods::PyBufferProcs> {
                    static PROCS: pyo3::class::proto_methods::PyBufferProcs
                        = pyo3::class::proto_methods::PyBufferProcs {
                            bf_getbuffer: Some(pyo3::class::buffer::getbuffer::<#ty>),
                            bf_releasebuffer: Some(pyo3::class::buffer::releasebuffer::<#ty>),
                        };
                    Some(&PROCS)
                }
            }
        });
    }

    let mut tokens = proto
        .slot_defs(method_names)
        .map(|def| {
            let slot = syn::Ident::new(def.slot, Span::call_site());
            let slot_impl: syn::Path = syn::parse_str(def.slot_impl).unwrap();
            quote! {{
                pyo3::ffi::PyType_Slot {
                    slot: pyo3::ffi::#slot,
                    pfunc: #slot_impl::<#ty> as _
                }
            }}
        })
        .peekable();

    if tokens.peek().is_none() {
        return Ok(quote! {});
    }

    Ok(quote! {
        impl #slots_trait<#ty> for pyo3::class::proto_methods::PyClassProtocols<#ty> {
            fn #slots_trait_slots(self) -> &'static [pyo3::ffi::PyType_Slot] {
                &[#(#tokens),*]
            }
        }
    })
}
