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
const completion_1 = require("../../src/lsp/completion");
suite('Completion Test Suite', () => {
    test('test_completion_empty_prefix', () => {
        const items = (0, completion_1.getCompletions)('');
        assert.ok(items.length >= 18);
    });
    test('test_completion_prefix_f', () => {
        const items = (0, completion_1.getCompletions)('f');
        const labels = items.map((i) => i.label);
        assert.ok(labels.includes('fn'));
        assert.ok(labels.includes('for'));
        assert.ok(labels.includes('false'));
    });
    test('test_completion_prefix_le', () => {
        const items = (0, completion_1.getCompletions)('le');
        const labels = items.map((i) => i.label);
        assert.ok(labels.includes('let'));
    });
    test('test_completion_no_match', () => {
        const items = (0, completion_1.getCompletions)('zzz');
        assert.strictEqual(items.length, 0);
    });
    test('test_keyword_docs_fn', () => {
        const docs = (0, completion_1.getKeywordDocs)('fn');
        assert.ok(docs !== null);
        assert.ok(docs.includes('fn'));
    });
    test('test_keyword_docs_unknown', () => {
        const docs = (0, completion_1.getKeywordDocs)('notakeyword');
        assert.strictEqual(docs, null);
    });
});
//# sourceMappingURL=completion.test.js.map