// import fs from 'fs/promises';

// const data = await fs.readFile('./example.js', 'utf8');
// console.log(data);


import fs from 'fs/promises';
import path from 'path';

import d3dag from 'd3-dag';
import lorix from 'lorix';
import task from 'tasuku';
import lodash from 'lodash';


function fnSignatureMatch(fnString) {
    const parentTransformRegex = /Input\(\s*"(.*?)"\s*/g;
    let parentMatches = [];
    for(let match of [...fnString.matchAll(parentTransformRegex)]) {
        // console.log(match[1]);
        parentMatches.push(match[1]); // Append matched group (representing parent transform)
    }
    return parentMatches;
}


async function walk(dir) {
    // Convert to relative path
    if (path.isAbsolute(dir)) {
        dir = path.relative('', dir); // Compare to cwd
    }

    let files = await fs.readdir(dir);
    files = await Promise.all(files.map(async file => {
        const filePath = path.join(dir, file);
        const stats = await fs.stat(filePath);
        if (stats.isDirectory()) return walk(filePath); // TODO check if recursive dir walk is wanted by user
        else if(stats.isFile()) return filePath;
    }));

    return files.reduce((all, folderContents) => all.concat(folderContents), []);
}


let files = await walk('./src');
// let files = await walk('../src');
// let files = await walk('/home/jms/dev/src');

// Dynamic module loading
// TODO: error handling
let transformMaster = [];
let transformLayering = [];
let testicle = {};


for (let transformFile of files) {
    let transformModule = await import('./' + transformFile);
    for(let name in transformModule) {
        // TODO: check that name is a function starting with "transform_"

        // Construct DAG node for layering / ordering
        let transformLayeringDef = {};
        let parents = fnSignatureMatch(transformModule[name].toString());
        transformLayeringDef["id"] = name;
        if (parents.length) {
            transformLayeringDef["parentIds"] = [];
            for (let match of parents) {
                transformLayeringDef["parentIds"].push(match);
            }
        }
        transformLayering.push(transformLayeringDef);

        // Construct DAG node for execution
        transformMaster.push({"name": name, "fn": transformModule[name], "input": transformLayeringDef["parentIds"]});
        testicle[name] = transformModule;
    }
}

let transformMasterDf = lorix.DataFrame.fromArray(transformMaster)
console.log(transformLayering);
console.log();


/*
    CREATE DAG FROM TRANSFORM MASTER

    - Iterate through transformLayering and create DAG step-by-step
    - DAG creation should error if a cycle is detected in process of creation
    - Once DAG is created with no errors, find root nodes (there can be more than one root), and start executing
*/

let stratify = d3dag.dagStratify();
let dag = stratify(transformLayering);
const layout = d3dag.sugiyama();
layout(dag);

// console.log(dag);
console.log("roots:");
console.log(dag.roots());
console.log();

console.log("nodes:");
for (let node of dag) {
    console.log(node);
}
console.log();

// Convert to DataFrames and execute layers in parallel
let transformArr = [...dag].map(node => {
    return {
        "name": node["data"]["id"],
        "layer": node["value"]
    };
});

let layerDf = lorix.DataFrame.fromArray(transformArr);
let df = (
    transformMasterDf
    .leftJoin(layerDf, ["name"])
    .orderBy(["layer"])
);
df.head();




/*
    EXECUTE DAG

    - Parallelize execution per layer (using layout created "value" property in each node)
    -
*/

let transformMap = df.groupBy(["layer"])  // Returns Map
let resultAggregator = {};

function getTaskDef(trfm, exec) {
    // TODO: get inputs
    let inputAggregator = [];
    if (trfm["input"] != undefined) {
        for (let input of trfm["input"]) {
            // Find result (if any)
            if (input in resultAggregator) {
                inputAggregator.push(resultAggregator[input]);
            } else {
                inputAggregator.push(undefined);
            }
        }
    }
    return exec(trfm["name"], async () => await trfm["fn"](...inputAggregator));
}

async function getTaskExecFn(arr) {
    const result = await Promise.all(arr.map(trfm => getTaskDef(trfm, task)));
    return lodash.zip(arr, result);
}

for (let level of transformMap) {
    // console.log(level[1]);
    let results = await getTaskExecFn(level[1]);
    // console.log(results);
    // Store results
    for (let result of results) {
        if (result[1]["result"] != undefined) {
            console.log(result[0]["name"], result[1]["result"]);
            resultAggregator[result[0]["name"]] = result[1]["result"];
        }
    }
}



/* USING EVAL (TRY TO AVOID) */
// for (let layer of transformMap) {
//     // console.log(layer[1]);
//     // await execTransformsParallel(layer[1]);
//     // Construct eval string
//     let evalStr = "task.group(task => [";
//     for(let transform of layer[1]) {
//         evalStr = evalStr + ` task("${transform["name"]}", async () => await testicle["${transform["name"]}"].${transform["name"]}(),), `
//     }
//     evalStr = evalStr + `], {concurrency: ${layer[1].length}})`;
//     console.log(evalStr);
//     console.log();
//     eval(evalStr);
// }
