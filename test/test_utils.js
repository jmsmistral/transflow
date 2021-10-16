import chai from "chai";
const { expect } = chai;

import {
    _isValidString,
    _validateString
 } from "../src/utils.js";


describe("Utils utility library", () => {

    describe("_isValidString()", () => {

        it("Should return true for non-empty strings", () => {
            const validString = "someString";
            expect(_isValidString(validString)).to.be.true;
        });

        it("Should return false for empty strings and other types", () => {
            const invalidString1 = "";
            const invalidString2 = undefined;
            const invalidString3 = 1;
            const invalidString4 = ["testArray"];
            expect(_isValidString(invalidString1)).to.be.false;
            expect(_isValidString(invalidString2)).to.be.false;
            expect(_isValidString(invalidString3)).to.be.false;
            expect(_isValidString(invalidString4)).to.be.false;
        });

    });


    describe("_validateString()", () => {

        it("Should return string for non-empty strings", () => {
            const validString = "someString";
            expect(_validateString(validString)).to.equal(validString);
        });

        it("Should throw an error for empty strings and other types", () => {
            const invalidString1 = "";
            const invalidString2 = undefined;
            const invalidString3 = 1;
            const invalidString4 = ["testArray"];
            expect(() => _validateString(invalidString1, "Error Message")).to.throw();
            expect(() => _validateString(invalidString2, "Error Message")).to.throw();
            expect(() => _validateString(invalidString3, "Error Message")).to.throw();
            expect(() => _validateString(invalidString4, "Error Message")).to.throw();
        });

    });

});
