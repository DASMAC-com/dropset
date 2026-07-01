/* @ts-self-types="./dropset_interface.d.ts" */

/**
 * Result of [`simulate_swap`].
 */
export class Quote {
    static __wrap(ptr) {
        ptr = ptr >>> 0;
        const obj = Object.create(Quote.prototype);
        obj.__wbg_ptr = ptr;
        QuoteFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        QuoteFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_quote_free(ptr, 0);
    }
    /**
     * @returns {bigint}
     */
    get fee_amount() {
        const ret = wasm.quote_fee_amount(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * @returns {bigint}
     */
    get in_amount() {
        const ret = wasm.quote_in_amount(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * @returns {number}
     */
    get legs() {
        const ret = wasm.quote_legs(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @returns {bigint}
     */
    get out_amount() {
        const ret = wasm.quote_out_amount(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
}
if (Symbol.dispose) Quote.prototype[Symbol.dispose] = Quote.prototype.free;

/**
 * `quote / price`, rounded toward zero (saturated to u64).
 * @param {number} bits
 * @param {bigint} quote
 * @returns {bigint}
 */
export function price_base_for_quote(bits, quote) {
    const ret = wasm.price_base_for_quote(bits, quote);
    return BigInt.asUintN(64, ret);
}

/**
 * Decode raw `Price` bits to a number (`0` / `Infinity` for sentinels).
 * @param {number} bits
 * @returns {number}
 */
export function price_decode(bits) {
    const ret = wasm.price_decode(bits);
    return ret;
}

/**
 * Encode a decimal price (e.g. `1.085`) to raw `Price` bits, or `None`
 * (JS `undefined`) if out of range.
 * @param {number} value
 * @returns {number | undefined}
 */
export function price_encode(value) {
    const ret = wasm.price_encode(value);
    return ret === 0x100000001 ? undefined : ret;
}

/**
 * Whether `bits` is a valid `Price` encoding.
 * @param {number} bits
 * @returns {boolean}
 */
export function price_is_valid(bits) {
    const ret = wasm.price_is_valid(bits);
    return ret !== 0;
}

/**
 * `base * price`, rounded toward zero (saturated to u64).
 * @param {number} bits
 * @param {bigint} base
 * @returns {bigint}
 */
export function price_quote_for_base(bits, base) {
    const ret = wasm.price_quote_for_base(bits, base);
    return BigInt.asUintN(64, ret);
}

/**
 * Simulate a take against a market account's raw data (including the
 * 8-byte discriminator). `side`: 0 = buy, 1 = sell. `limit_price_bits`:
 * raw `Price` bits (use the per-side no-bound sentinel to disable).
 * @param {Uint8Array} market_data
 * @param {number} side
 * @param {bigint} amount_in
 * @param {number} limit_price_bits
 * @param {number} current_slot
 * @returns {Quote}
 */
export function simulate_swap(market_data, side, amount_in, limit_price_bits, current_slot) {
    const ptr0 = passArray8ToWasm0(market_data, wasm.__wbindgen_malloc);
    const len0 = WASM_VECTOR_LEN;
    const ret = wasm.simulate_swap(ptr0, len0, side, amount_in, limit_price_bits, current_slot);
    if (ret[2]) {
        throw takeFromExternrefTable0(ret[1]);
    }
    return Quote.__wrap(ret[0]);
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_960c155d3d49e4c2: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg___wbindgen_throw_6b64449b9b9ed33c: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./dropset_interface_bg.js": import0,
    };
}

const QuoteFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_quote_free(ptr >>> 0, 1));

function getStringFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return decodeText(ptr, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasm;
function __wbg_finalize_init(instance, module) {
    wasm = instance.exports;
    wasmModule = module;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('dropset_interface_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
