"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
const assert = __importStar(require("assert"));
const diagnostics_1 = require("../../src/lsp/diagnostics");
const vscode_languageserver_textdocument_1 = require("vscode-languageserver-textdocument");
function makeDoc(text) {
    return vscode_languageserver_textdocument_1.TextDocument.create('file:///test.nuzo', 'nuzo', 1, text);
}
suite('Diagnostics Test Suite', () => {
    test('test_parse_error_format1', () => {
        const doc = makeDoc('let x = ;\n');
        const diags = (0, diagnostics_1.parseCompilerErrors)('error: unexpected token at line 1:col 10', doc);
        assert.strictEqual(diags.length, 1);
        assert.strictEqual(diags[0].range.start.line, 0);
    });
    test('test_parse_error_format2', () => {
        const doc = makeDoc('let x = 1;\nlet y = ;\n');
        const diags = (0, diagnostics_1.parseCompilerErrors)('error: unexpected token (line 2)', doc);
        assert.strictEqual(diags.length, 1);
        assert.strictEqual(diags[0].range.start.line, 1);
    });
    test('test_parse_error_format3', () => {
        const doc = makeDoc('fn main() {\n  invalid\n}\n');
        const diags = (0, diagnostics_1.parseCompilerErrors)('test.nuzo:2:3: error: unknown identifier', doc);
        assert.strictEqual(diags.length, 1);
        assert.strictEqual(diags[0].range.start.line, 1);
        assert.strictEqual(diags[0].range.start.character, 2);
    });
    test('test_parse_no_errors', () => {
        const doc = makeDoc('let x = 1;\n');
        const diags = (0, diagnostics_1.parseCompilerErrors)('', doc);
        assert.strictEqual(diags.length, 0);
    });
    test('test_parse_fallback_raw_stderr', () => {
        const doc = makeDoc('code\n');
        const diags = (0, diagnostics_1.parseCompilerErrors)('some unstructured error output', doc);
        assert.strictEqual(diags.length, 1);
        assert.ok(diags[0].message.includes('unstructured'));
    });
});
//# sourceMappingURL=diagnostics.test.js.map