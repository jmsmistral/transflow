import task from 'tasuku';

import {
    _validateString,
    _getFileNamesInDirectory,
    _importFilesAsModules,
    _getTransformFunctionsFromModules,
    _getDagFromFunctions,
    _getDagExecutionOrderFromFunctions,
    _getTaskExecFn
} from './utils.js'


export class Workflow {
    // Workflow API
    // ------------
    // + addDagFromModule(dagName: string, filepath: string): returns an instance of a DAG from a Javascript module file
    // + addDagFromDirectory(dagName: string, filepath: string, recursive: boolean): returns an instance of a DAG from all Javascript modules found in the directory
    // + addDagFromFunctionArray(dagName: string, [functions]: array): returns an instance of a DAG from an array of functions


    constructor(workflowName) {
        this.name = _validateString(workflowName, `Invalid string passed as workflow name: ${workflowName}`);
        this.transforms = {};
        this.dags = {};
    }

    async addDagFromDirectory(dagName, dir, recursive=false) {
        /*
            Adds a DAG to the Workflow instance, from the set
            of Transform functions defined in Javascript Modules
            found in the specified directory `dir`. If the
            `recursive` flag is set, sub-directories will also be
            traversed.

            Throws an error if:
            - DAG with name `dagName` already exists in the Workflow.
            - The current directory is specified (as importing this would
              break things).
        */

        // Break if attempting to import from program root dir,
        // as importing the executing module will break things!
        if (dir === ".")
            throw Error("Attempting to import modules from the root directory will result in errors.")

        if (!_validateString(dagName))
            throw Error(`Invalid DAG name: ${dagName}`);

        // 1. Get references to files in directory
        let filepaths = await _getFileNamesInDirectory(dir, recursive);
        // 2. Import all files matching javascript files as modules (*.js, *.mjs)
        let modules = await _importFilesAsModules(filepaths);
        // 3. Identify Transforms exported by modules ("transform_<name>(...)")
        let transforms = _getTransformFunctionsFromModules(modules);
        // 4. Generate DAG from imported Transforms
        let dag = _getDagFromFunctions(transforms);
        // 5. Calculate DAG node execution order
        let dagExecOrder = _getDagExecutionOrderFromFunctions(transforms);

        // 6. Add DAG to this.dags keyed by names (overwrite if DAG with same name already exists)
        this.dags[dagName] = dag.leftJoin(dagExecOrder, ["name"]);
    }

    async run(dags=[], parallel=false) {
        /*
            Executes the DAGs specified in `dags`, or
            all DAGs in the Workflow if none are specified.
            If the parallel flag is set, all transforms across
            all DAGs in the Workflow will be executed as if
            they belong to a single DAG.
        */
        for (let dag in this.dags) {

            await task(dag, async ({task}) => {

                let transformMap = this.dags[dag].groupBy(["order"]);  // Returns Map
                let resultAggregator = {};

                for (let execOrder of transformMap) {
                    let results = await _getTaskExecFn(task, execOrder[1], resultAggregator);
                    // Store results
                    for (let result of results)
                        if (result[1]["result"] != undefined)
                            resultAggregator[result[0]["name"]] = result[1]["result"];
                }

            })

        }

    }


}
