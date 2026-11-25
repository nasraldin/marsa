use core_foundation::{
    array::{
        CFArray, CFArrayAppendValue, CFArrayCreateMutable, CFMutableArrayRef, kCFTypeArrayCallBacks,
    },
    base::{CFRelease, TCFType, kCFAllocatorDefault},
    dictionary::{
        CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
    },
    number::CFNumber,
    string::CFString,
};
use core_foundation_sys::locale::CFLocaleCopyPreferredLanguages;
use core_text::font_descriptor::{
    TraitAccessors, kCTFontFamilyNameAttribute, kCTFontItalicTrait, kCTFontSlantTrait,
    kCTFontTraitsAttribute, kCTFontWeightTrait,
};
use core_text::{
    font::{CTFont, cascade_list_for_languages},
    font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCreateCopyWithVariation,
        CTFontDescriptorCreateWithAttributes, kCTFontCascadeListAttribute,
        kCTFontFeatureSettingsAttribute,
    },
};
use font_kit::font::Font as FontKitFont;
use gpui::{FontFallbacks, FontFeatures};
use objc2_core_text::{
    kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue, kCTFontVariationAxisIdentifierKey,
    kCTFontVariationAxisMaximumValueKey, kCTFontVariationAxisMinimumValueKey,
};

pub fn apply_features_and_fallbacks(
    font: &mut FontKitFont,
    features: &FontFeatures,
    fallbacks: Option<&FontFallbacks>,
) -> anyhow::Result<()> {
    unsafe {
        let mut keys = vec![kCTFontFeatureSettingsAttribute];
        let mut values = vec![generate_feature_array(features)];
        if let Some(fallbacks) = fallbacks
            && !fallbacks.fallback_list().is_empty()
        {
            keys.push(kCTFontCascadeListAttribute);
            values.push(generate_fallback_array(fallbacks, font));
        }
        let attrs = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr() as _,
            values.as_ptr() as _,
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        for value in &values {
            CFRelease(*value as _);
        }

        let new_descriptor = CTFontDescriptorCreateWithAttributes(attrs);
        CFRelease(attrs as _);
        let new_descriptor = CTFontDescriptor::wrap_under_create_rule(new_descriptor);
        *font = copy_with_descriptor(&font.native_font(), &new_descriptor);

        Ok(())
    }
}

/// Apply an OpenType CSS weight to a variable family loaded from memory.
pub(crate) fn with_weight_variation(font: &FontKitFont, weight: f32) -> Option<FontKitFont> {
    let native_font = font.native_font();
    let axes = native_font.get_variation_axes()?;
    let weight_tag = i64::from(u32::from_be_bytes(*b"wght"));
    // SAFETY: CoreText supplies process-lifetime CFString constants for the axis dictionary.
    let (identifier, minimum, maximum) = unsafe {
        (
            CFString::wrap_under_get_rule(
                std::ptr::from_ref(kCTFontVariationAxisIdentifierKey).cast(),
            ),
            CFString::wrap_under_get_rule(
                std::ptr::from_ref(kCTFontVariationAxisMinimumValueKey).cast(),
            ),
            CFString::wrap_under_get_rule(
                std::ptr::from_ref(kCTFontVariationAxisMaximumValueKey).cast(),
            ),
        )
    };
    let axis = axes.iter().find(|axis| {
        axis.find(&identifier)
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|value| value.to_i64())
            == Some(weight_tag)
    })?;
    let minimum = axis.find(&minimum)?.downcast::<CFNumber>()?.to_f64()?;
    let maximum = axis.find(&maximum)?.downcast::<CFNumber>()?.to_f64()?;
    if !weight.is_finite()
        || minimum
            .partial_cmp(&maximum)
            .is_none_or(|order| order.is_gt())
    {
        return None;
    }
    let tag = CFNumber::from(weight_tag);
    let descriptor = native_font.copy_descriptor();
    // SAFETY: The descriptor and CFNumber are live and the font advertises the weight axis.
    let variant = unsafe {
        CTFontDescriptorCreateCopyWithVariation(
            descriptor.as_concrete_TypeRef(),
            tag.as_concrete_TypeRef(),
            f64::from(weight).clamp(minimum, maximum),
        )
    };
    if variant.is_null() {
        return None;
    }
    // SAFETY: CoreText returned an owned descriptor under the Create rule.
    let variant = unsafe { CTFontDescriptor::wrap_under_create_rule(variant) };
    Some(copy_with_descriptor(&native_font, &variant))
}

fn copy_with_descriptor(native_font: &CTFont, descriptor: &CTFontDescriptor) -> FontKitFont {
    // SAFETY: The legacy wrappers and generated bindings refer to the same typed CF objects.
    // Transfer the retained font from the generated API back to the legacy create-rule wrapper.
    unsafe {
        let generated_font = &*native_font
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFont>();
        let descriptor = &*descriptor
            .as_concrete_TypeRef()
            .cast::<objc2_core_text::CTFontDescriptor>();
        let new_font = generated_font.copy_with_attributes(0.0, std::ptr::null(), Some(descriptor));
        let new_font = CTFont::wrap_under_create_rule(
            objc2_core_foundation::CFRetained::into_raw(new_font)
                .as_ptr()
                .cast(),
        );
        FontKitFont::from_native_font(&new_font)
    }
}

fn generate_feature_array(features: &FontFeatures) -> CFMutableArrayRef {
    unsafe {
        let feature_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for (tag, value) in features.tag_value_list() {
            let keys = [
                kCTFontOpenTypeFeatureTag as *const _,
                kCTFontOpenTypeFeatureValue as *const _,
            ];
            let tag = CFString::new(tag);
            let value = CFNumber::from(*value as i32);
            let values = [tag.as_CFTypeRef(), value.as_CFTypeRef()];
            let dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                &keys as *const _ as _,
                &values as *const _ as _,
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFArrayAppendValue(feature_array, dict as _);
            CFRelease(dict as _);
        }
        feature_array
    }
}

fn generate_fallback_array(fallbacks: &FontFallbacks, font: &mut FontKitFont) -> CFMutableArrayRef {
    unsafe {
        let symbolic_traits = font.native_font().symbolic_traits();
        let all_traits = font.native_font().all_traits();

        let fallback_array = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
        for user_fallback in fallbacks.fallback_list() {
            let name = CFString::from(user_fallback.as_str());

            let traits_keys = [kCTFontWeightTrait, kCTFontSlantTrait];
            let weight_value = CFNumber::from(all_traits.normalized_weight());
            let slant_value = CFNumber::from(if (symbolic_traits & kCTFontItalicTrait) != 0 {
                1.0
            } else {
                0.0
            });
            let traits_values = [weight_value.as_CFTypeRef(), slant_value.as_CFTypeRef()];
            let traits = CFDictionaryCreate(
                kCFAllocatorDefault,
                &traits_keys as *const _ as _,
                &traits_values as *const _ as _,
                traits_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            drop(weight_value);
            drop(slant_value);

            let attr_keys = [kCTFontFamilyNameAttribute, kCTFontTraitsAttribute];
            let attr_values = [name.as_CFTypeRef(), traits as _];
            let attrs = CFDictionaryCreate(
                kCFAllocatorDefault,
                &attr_keys as *const _ as _,
                &attr_values as *const _ as _,
                attr_keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            CFRelease(traits as _);

            let fallback_desc = CTFontDescriptorCreateWithAttributes(attrs);
            CFRelease(attrs as _);

            CFArrayAppendValue(fallback_array, fallback_desc as _);
            CFRelease(fallback_desc as _);
        }

        append_system_fallbacks(fallback_array, &font.native_font());
        fallback_array
    }
}

fn append_system_fallbacks(fallback_array: CFMutableArrayRef, font: &CTFont) {
    unsafe {
        let preferred_languages: CFArray<CFString> =
            CFArray::wrap_under_create_rule(CFLocaleCopyPreferredLanguages());

        let default_fallbacks = cascade_list_for_languages(font, &preferred_languages);

        for desc in default_fallbacks
            .iter()
            .filter(|desc| desc.font_path().is_some())
        {
            CFArrayAppendValue(fallback_array, desc.as_concrete_TypeRef() as _);
        }
    }
}
