import {
    Workflow
} from "./src/workflow.js";


import {
    _validateString,
    _getFileNamesInDirectory
} from './src/utils.js'


let wf = new Workflow("workflow 1");
// await wf.addDagFromFilepath("dag 1", "."); // Error
await wf.addDagFromDirectory("dag 1", "./transforms");
await wf.run();
// console.log(wf);
