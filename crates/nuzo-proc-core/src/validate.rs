//! 编译期校验工具
//!
//! 提供重复属性检测、字段类型校验、where 子句生成、标识符保留字校验、
//! 可见性校验、字段名收集等工具。

use quote::quote;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisKind {
    Public,
    Private,
    Any,
}

pub fn validate_no_duplicate_attrs(attrs: &[syn::Attribute], attr_name: &str) -> syn::Result<()> {
    let mut seen = false;
    for attr in attrs {
        if attr.path().is_ident(attr_name) {
            if seen {
                return Err(syn::Error::new_spanned(
                    attr,
                    format!("duplicate attribute `{attr_name}`"),
                ));
            }
            seen = true;
        }
    }
    Ok(())
}

pub fn validate_field_types(fields: &syn::Fields, allowed: &[&str]) -> Vec<syn::Result<syn::Type>> {
    fields
        .iter()
        .map(|field| {
            let ty = &field.ty;
            let type_str = quote!(#ty).to_string().replace(' ', "");
            if allowed.iter().any(|a| a.replace(' ', "") == type_str) {
                Ok(field.ty.clone())
            } else {
                Err(syn::Error::new_spanned(
                    ty,
                    format!(
                        "type `{}` is not allowed; expected one of: {}",
                        type_str,
                        allowed.join(", ")
                    ),
                ))
            }
        })
        .collect()
}

pub fn generate_where_clause(
    generics: &syn::Generics,
    bounds: &[syn::TypeParamBound],
) -> proc_macro2::TokenStream {
    if bounds.is_empty() {
        return proc_macro2::TokenStream::new();
    }
    let type_params: Vec<_> = generics.type_params().map(|tp| &tp.ident).collect();
    if type_params.is_empty() {
        return proc_macro2::TokenStream::new();
    }
    let predicates: Vec<proc_macro2::TokenStream> =
        type_params.iter().map(|ident| quote!(#ident: #(#bounds)+*)).collect();
    quote!(where #(#predicates),*)
}

pub fn validate_ident_not_reserved(
    ident: &proc_macro2::Ident,
    reserved: &[&str],
) -> syn::Result<()> {
    let name = ident.to_string();
    if reserved.iter().any(|r| *r == name) {
        Err(syn::Error::new_spanned(ident, format!("identifier `{name}` is reserved")))
    } else {
        Ok(())
    }
}

pub fn validate_vis(vis: &syn::Visibility, expected: VisKind) -> syn::Result<()> {
    match expected {
        VisKind::Any => Ok(()),
        VisKind::Public => match vis {
            syn::Visibility::Public(_) => Ok(()),
            _ => Err(syn::Error::new_spanned(vis, "expected `pub` visibility")),
        },
        VisKind::Private => match vis {
            syn::Visibility::Public(_) => {
                Err(syn::Error::new_spanned(vis, "expected private visibility"))
            }
            _ => Ok(()),
        },
    }
}

pub fn collect_field_names(fields: &syn::Fields) -> Vec<proc_macro2::Ident> {
    match fields {
        syn::Fields::Named(named) => named.named.iter().filter_map(|f| f.ident.clone()).collect(),
        syn::Fields::Unnamed(_) | syn::Fields::Unit => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn no_duplicate_ok() {
        let item: syn::ItemStruct = parse_quote! {
            #[my_attr]
            struct Foo {}
        };
        assert!(validate_no_duplicate_attrs(&item.attrs, "my_attr").is_ok());
    }

    #[test]
    fn duplicate_err() {
        let item: syn::ItemStruct = parse_quote! {
            #[my_attr]
            #[my_attr]
            struct Foo {}
        };
        assert!(validate_no_duplicate_attrs(&item.attrs, "my_attr").is_err());
    }

    #[test]
    fn different_attr_names_ok() {
        let item: syn::ItemStruct = parse_quote! {
            #[a]
            #[b]
            struct Foo {}
        };
        assert!(validate_no_duplicate_attrs(&item.attrs, "a").is_ok());
        assert!(validate_no_duplicate_attrs(&item.attrs, "b").is_ok());
    }

    #[test]
    fn no_matching_attr_ok() {
        let item: syn::ItemStruct = parse_quote! {
            #[other]
            struct Foo {}
        };
        assert!(validate_no_duplicate_attrs(&item.attrs, "my_attr").is_ok());
    }

    #[test]
    fn field_types_all_allowed() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo { x: i32, y: String }
        };
        let results = validate_field_types(&item.fields, &["i32", "String"]);
        assert!(results.iter().all(|r| r.is_ok()));
    }

    #[test]
    fn field_types_some_disallowed() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo { x: i32, y: bool }
        };
        let results = validate_field_types(&item.fields, &["i32"]);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
    }

    #[test]
    fn field_types_empty_allowed() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo { x: i32 }
        };
        let results = validate_field_types(&item.fields, &[]);
        assert!(results[0].is_err());
    }

    #[test]
    fn field_types_unnamed_ignored() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo(i32, bool);
        };
        let results = validate_field_types(&item.fields, &["i32"]);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
    }

    #[test]
    fn where_clause_single_param_single_bound() {
        let generics: syn::Generics = parse_quote!(<T>);
        let bound: syn::TypeParamBound = parse_quote!(Clone);
        let result = generate_where_clause(&generics, &[bound]);
        let expected = quote!(where T: Clone);
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn where_clause_multi_param_multi_bound() {
        let generics: syn::Generics = parse_quote!(<T, U>);
        let bounds: Vec<syn::TypeParamBound> = vec![parse_quote!(Clone), parse_quote!(Debug)];
        let result = generate_where_clause(&generics, &bounds);
        let expected = quote!(where T: Clone + Debug, U: Clone + Debug);
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn where_clause_no_type_params() {
        let generics: syn::Generics = parse_quote!();
        let bound: syn::TypeParamBound = parse_quote!(Clone);
        let result = generate_where_clause(&generics, &[bound]);
        assert!(result.is_empty());
    }

    #[test]
    fn where_clause_no_bounds() {
        let generics: syn::Generics = parse_quote!(<T>);
        let result = generate_where_clause(&generics, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn where_clause_skips_lifetime_params() {
        let generics: syn::Generics = parse_quote!(<'a, T>);
        let bound: syn::TypeParamBound = parse_quote!(Clone);
        let result = generate_where_clause(&generics, &[bound]);
        let expected = quote!(where T: Clone);
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn ident_not_reserved_ok() {
        let ident: proc_macro2::Ident = parse_quote!(foo);
        assert!(validate_ident_not_reserved(&ident, &["bar", "baz"]).is_ok());
    }

    #[test]
    fn ident_reserved_err() {
        // "self" 是 Rust 关键字，无法通过 parse_quote! 生成 Ident，
        // 使用 proc_macro2::Ident::new 直接构造以绕过关键字检查
        let ident = proc_macro2::Ident::new("self", proc_macro2::Span::call_site());
        assert!(validate_ident_not_reserved(&ident, &["self", "super"]).is_err());
    }

    #[test]
    fn ident_empty_reserved_ok() {
        let ident: proc_macro2::Ident = parse_quote!(foo);
        assert!(validate_ident_not_reserved(&ident, &[]).is_ok());
    }

    #[test]
    fn vis_public_expected_public_ok() {
        let vis: syn::Visibility = parse_quote!(pub);
        assert!(validate_vis(&vis, VisKind::Public).is_ok());
    }

    #[test]
    fn vis_public_expected_private_err() {
        let vis: syn::Visibility = parse_quote!(pub);
        assert!(validate_vis(&vis, VisKind::Private).is_err());
    }

    #[test]
    fn vis_inherited_expected_private_ok() {
        assert!(validate_vis(&syn::Visibility::Inherited, VisKind::Private).is_ok());
    }

    #[test]
    fn vis_inherited_expected_public_err() {
        assert!(validate_vis(&syn::Visibility::Inherited, VisKind::Public).is_err());
    }

    #[test]
    fn vis_restricted_not_public() {
        let item: syn::ItemStruct = parse_quote! { pub(crate) struct Foo {} };
        assert!(validate_vis(&item.vis, VisKind::Public).is_err());
        assert!(validate_vis(&item.vis, VisKind::Private).is_ok());
    }

    #[test]
    fn vis_any_accepts_all() {
        let pub_vis: syn::Visibility = parse_quote!(pub);
        assert!(validate_vis(&pub_vis, VisKind::Any).is_ok());
        assert!(validate_vis(&syn::Visibility::Inherited, VisKind::Any).is_ok());
    }

    #[test]
    fn collect_named() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo { x: i32, y: String }
        };
        let names = collect_field_names(&item.fields);
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].to_string(), "x");
        assert_eq!(names[1].to_string(), "y");
    }

    #[test]
    fn collect_unnamed_empty() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo(i32, String);
        };
        let names = collect_field_names(&item.fields);
        assert!(names.is_empty());
    }

    #[test]
    fn collect_unit_empty() {
        let item: syn::ItemStruct = parse_quote! {
            struct Foo;
        };
        let names = collect_field_names(&item.fields);
        assert!(names.is_empty());
    }
}
