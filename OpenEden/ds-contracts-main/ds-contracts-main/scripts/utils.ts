import { BaseContract, Signer } from 'ethers';
import { ethers } from 'hardhat'
import hre from "hardhat";
import fs from "fs";

export async function load(name: string) {
    try {
        const data = await fs.readFileSync(`${process.cwd()}/addresses/${name}.json`)
        return JSON.parse(data.toString())
    } catch (e) {
        console.log(`No ${name} file to load, instead creating new one`)
        return {}
    }
}

export async function save(name: string, content: any) {
    const sharedAddressPath = `${process.cwd()}/addresses/${name}.json`
    await fs.writeFileSync(sharedAddressPath, JSON.stringify(content, null, 2))
}

export async function deployContract<T extends BaseContract>(
    contractName: string, 
    args: any[] = [],
    signer: Signer|undefined = undefined, 
    isTest = false
): Promise<T>{
    console.log(`----------------------------Deploy ${contractName} --------------------------------`);

    const ContractFactory = await ethers.getContractFactory(contractName, signer);

    const contract: T = (await ContractFactory.deploy(...args)) as T;
    // console.log(`${contractName}: ${contract.deployTransaction.hash}`);

    await contract.deployed();

    console.log(`${contractName} deployed to:`, contract.address);
    if (!isTest) {
        await save(contractName, {
            address: contract.address,
            args: args
        });
    }
    return contract;
};

export async function verifyContract (
    contractName: string
) {
    console.log(`----------------------------Verify ${contractName} --------------------------------`);
    const contractDeploymentInfo = await load(contractName);
    const contractAddress = contractDeploymentInfo.address
    const contractArgs = contractDeploymentInfo.args
    await hre.run("verify:verify", {
        address: contractAddress,
        constructorArguments: [
            ...contractArgs
        ],
    });
    
    console.log(`---------------------------Verified ${contractName}----------------------------------`);
};