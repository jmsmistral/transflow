import fs from 'fs/promises';
import path from 'path';
import url from 'url';

import lorix from 'lorix';
import d3dag from 'd3-dag';
// import task from 'tasuku';
import lodash from 'lodash';


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


export function _getTransformParentsFromFuncSignature(fnString) {
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


export function _getTransformFunctionsFromModules(modules) {
    /*
        Returns a mapping of function name-to-function exported
        from `modules`.
    */

    let transforms = {};
    for (let moduleDef in modules)
        for (let transformName in modules[moduleDef])
            if (transformName.match(/transform_/g).length) {
                if (transformName in transforms)
                    throw Error(`Duplicate Transform name detected: ${transformName}`)
                transforms[transformName] = modules[moduleDef][transformName];
            }

    return transforms;
}


export function _getDagFromFunctions(transforms) {
    /*
        Returns DAG DataFrame from Array of Transforms `transforms`.
    */

    let dag = [];

    for (let transformName in transforms) {
        let parentReferences = _getTransformParentsFromFuncSignature(transforms[transformName].toString());
        let parents = [];
        if (parentReferences.length)
            for (let match of parentReferences) parents.push(match);

        // Add node to DAG for execution
        dag.push({
            "name": transformName,
            "fn": transforms[transformName],
            "input": parents
        });
    }

    return lorix.DataFrame.fromArray(dag);
}



export function _getDagExecutionOrderFromFunctions(transforms) {
    /*
        Returns a DataFrame with an integer assigned to each
        DAG node, representing the order of execution for that node.
        Nodes with the same order of execution will be executed
        in parallel.
    */

    let dag = [];

    for (let transformName in transforms) {
        let transform = {"id": transformName};
        let parents = _getTransformParentsFromFuncSignature(transforms[transformName].toString());
        if (parents.length) {
            transform["parentIds"] = [];
            for (let match of parents) transform["parentIds"].push(match);
        }
        dag.push(transform);
    }

    // Calculate node ordering
    const createDagStructure = d3dag.dagStratify();
    const generateExecutionOrder = d3dag.sugiyama();
    let dagExecOrder = createDagStructure(dag);
    generateExecutionOrder(dagExecOrder);

    // Convert to DataFrame
    let dfArray = [...dagExecOrder].map(node => {
        return {
            "name": node["data"]["id"],
            "order": node["value"]
        };
    });

    return lorix.DataFrame.fromArray(dfArray);
}


export function _getTaskDef(trfm, exec, resultAggregator) {
    // get inputs
    let inputAggregator = [];
    if (trfm["input"] != undefined) {
        for (let input of trfm["input"]) {
            // Find results of dependencies (if any)
            if (input in resultAggregator) {
                inputAggregator.push(resultAggregator[input]);
            } else {
                inputAggregator.push(undefined);
            }
        }
    }
    return exec(trfm["name"], async () => await trfm["fn"](...inputAggregator));
}

export async function _getTaskExecFn(task, arr, resultAggregator) {
    const result = await Promise.all(arr.map(trfm => _getTaskDef(trfm, task, resultAggregator)));
    return lodash.zip(arr, result);
}
