import {
    _validateString,
    _getFileNamesInDirectory,
    _importFilesAsModules,
    _getTransformFunctionsFromModules
} from './utils.js'


export class Workflow {
    // Workflow API
    // ------------
    // + addDagFromModule(dagName: string, filepath: string): returns an instance of a DAG from a Javascript module file
    // + addDagFromFilepath(dagName: string, filepath: string, recursive: boolean): returns an instance of a DAG from all Javascript modules found in the directory
    // + addDagFromFunctionArray(dagName: string, [functions]: array): returns an instance of a DAG from an array of functions


    constructor(workflowName) {
        this.name = _validateString(workflowName, `Invalid string passed as workflow name: ${workflowName}`);
        this.dags = {};
        this.transforms = {};
        this.transformsStratified = {};
    }

    async addDagFromFilepath(dagName, folderPath, recursive=false) {
        // Break if attempting to import from program root dir,
        // as importing the executing module will break things!
        if (folderPath === ".") {
            throw Error("Attempting to import modules from the root directory will result in errors.")
        }

        // 1. Get references to files in directory
        let filepaths = await _getFileNamesInDirectory(folderPath, recursive);
        // 2. Import all files matching javascript files as modules (*.js, *.mjs)
        let modules = await _importFilesAsModules(filepaths);
        // 3. Identify Transforms exported by modules ("transform_<name>(...)")
        let transforms = _getTransformFunctionsFromModules(modules);
        // 4. Generate DAG from imported Transforms
        // 5. Add DAG to this.dags keyed by names (overwrite if DAG with same name already exists)
    }

}
