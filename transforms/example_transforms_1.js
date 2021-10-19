import task from 'tasuku';
import lorix from 'lorix';

/* TASUKU EXAMPLE */
function sleep(ms) {
    return new Promise((resolve) => {
      setTimeout(resolve, ms);
    });
  }

async function someAsyncTask(n) {
    await sleep(n);
}

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


export async function transform_test1() {
    await someAsyncTask(2000);
    // console.log("result from transform_test1:");
    // return "SUP 1";
}

export async function transform_test2(
    test1=Input("transform_test1")
) {
    await someAsyncTask(2000);
    // console.log(test1.name);
    // console.log("result from transform_test2:");
    // console.log(test1);
    return test1;
}


// task("transform_test1", async () => {
//     await transform_test1();
//     // console.log(transform_test1.toString());
// });

// task("transform_test2", async () => {
//     await transform_test2();
// });
