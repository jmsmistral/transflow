import fs from 'fs/promises';
import path, { relative } from 'path';
import url from 'url';


export function _isValidString(str) {
    /*
        Checks whether `str` is a non-empty string.
    */

   return Boolean((Object.prototype.toString.call(str) === "[object String]") && (str.length));
}


export function _validateString(str, errorMsg) {
    /*
        Throws an error if `str` is not a valid string,
        returns `str` otherwise.
    */

    if (!_isValidString(str)) throw Error(errorMsg);
    return str;
}


export function _fnSignatureMatch(fnString) {
    const parentTransformRegex = /Input\(\s*"(.*?)"\s*/g;
    let parentMatches = [];
    for(let match of [...fnString.matchAll(parentTransformRegex)]) {
        // console.log(match[1]);
        parentMatches.push(match[1]); // Append matched group (representing parent transform)
    }
    return parentMatches;
}


export async function _getFileNamesInDirectory(dir, recursive=false) {
    /*
        Returns an array of file names in the specified directory `dir`.
        Recursively visits sub-directories if `recursive` is true.
        Throws an error if invalid directory is passed.
    */

    try {
        // Convert to relative path (to enable module import)
        if (path.isAbsolute(dir)) {
            dir = path.relative("", dir); // Compares to cwd
        }

        let files = await fs.readdir(dir);
        files = await Promise.all(files.map(async file => {
            const filePath = path.join(dir, file);
            const stats = await fs.stat(filePath);
            if ((stats.isDirectory()) && (recursive)) return _getFileNamesInFolder(filePath, recursive);
            else if(stats.isFile()) return filePath;
        }));

        // Remove undefined items from list
        files = files.filter(dirContents => { if (dirContents != undefined) return dirContents});

        return files.reduce((all, dirContents) => all.concat(dirContents), []);

    }
    catch(error) {
        throw Error(`Invalid path provided: ${dir}`);
    }
}


export async function _importFilesAsModules(files) {
    /*
        Returns a dictionary of file-imported module pairs -
        ignoring any files that are not Javascript modules.
        Throws an error if unable to import the module.
    */

    let modules = {};
    for (let modulePath of files) {
        try {
            if(![".js", ".mjs"].includes(path.extname(modulePath))) continue;
            // Determine path from this module to process root dir
            let relativePath = path.relative(path.dirname(url.fileURLToPath(import.meta.url)), process.cwd());
            let cdString = relativePath == "" ? "." : relativePath;
            modules[modulePath] = await import(path.join(cdString, modulePath));
        }
        catch (error) {
            throw Error(`Error attempting to load Javascript module: "${modulePath}"`);
        }
    }
    return modules;
}


// let files = await _getFileNamesInDirectory("./src/transforms", false);
// // let files = await _getFileNamesInDirectory(".", false);
// console.log(await _importFilesAsModules(files));


export async function _getTransformFunctionsFromModules(modules) {
    /*
        Returns an array of Transform functions exported
        from `modules`.
    */
    console.log(modules);

    for (let moduleDef in modules) {
        console.log(moduleDef);

        for (let transformFunction in moduleDef) {
           // TODO: check that transformFunction is a function starting with "transform_"
           console.log(transformFunction);

           // Construct DAG node for layering / ordering
           let transformLayeringDef = {};
           let parents = _fnSignatureMatch(moduleDef[transformFunction].toString());
           transformLayeringDef["id"] = transformFunction;
           if (parents.length) {
               transformLayeringDef["parentIds"] = [];
               for (let match of parents) {
                   transformLayeringDef["parentIds"].push(match);
               }
           }
           transformLayering.push(transformLayeringDef);

           // Construct DAG node for execution
           transformMaster.push({"name": transformFunction, "fn": moduleDef[transformFunction], "input": transformLayeringDef["parentIds"]});
       }

    }
}
