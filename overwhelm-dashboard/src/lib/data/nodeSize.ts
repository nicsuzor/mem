// Thin re-export shim. All emphasis logic now lives in focusEmphasis.ts —
// kept here for the existing import paths used by ForceView, CirclePackView,
// and TreemapView until they're migrated.
export {
    focusSize,
    maxFocusOf,
    FOCUS_SIZE_EXPONENT,
} from './focusEmphasis';
