import task from 'tasuku';
import lorix from 'lorix';

/* TASUKU EXAMPLE */
function sleep(ms) {
    return new Promise((resolve) => {
      setTimeout(resolve, ms);
    });
}

// async function someAsyncTask(n) {
//     await sleep(n);
// }

// task('Task 1', async () => {
//     await someAsyncTask(1000)
// });

// task('Task 2', async () => {
//     await someAsyncTask(2000)
// });

// task('Task 3', async () => {
//     await someAsyncTask(3000)
// });
/* END TASUKU EXAMPLE */

class Input_Impl{
    constructor(transform_name) {
        this.name = transform_name;
    }
}

function Input(transform_name) {
    return new Input_Impl(transform_name);
}

class Check_Impl{
    constructor(transform_name) {
        this.name = transform_name;
    }
}

function Check(transform_name) {
    return new Check_Impl(transform_name);
}


export function transform_test3() {
    // await sleep(1500);
    // console.log("i am transform_test3");
    console.log("result from transform_test3:")
}

export function transform_test4(
    test1=Input("transform_test1", Check("fucking check")),
    test2=Input("transform_test2")
) {
    // await sleep(2500);
    // console.log(test1.name);
    // console.log(test2.name);
    console.log("result from transform_test4:")
    console.log(test1);
    console.log(test2);
}


// task("transform_test1", async () => {
//     await transform_test1();
//     console.log(transform_test1.toString());
// });

// task("transform_test2", async () => {
//     await transform_test2();
// });
